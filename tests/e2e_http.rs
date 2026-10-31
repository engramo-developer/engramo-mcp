//! End-to-end tests for `engramo-mcp http`: each test spawns the real binary on a
//! loopback port, points it at a `wiremock` stand-in for the EngrAmo API, and talks to
//! it over a real socket — through rmcp's own Streamable HTTP client where a test
//! exercises the MCP protocol, and through raw `reqwest` where it asserts on HTTP
//! status codes and headers the rmcp client hides.
//!
//! The router-level tests in `main.rs` / `http_auth.rs` drive `build_app` in-process;
//! these cover what they can't: the CLI/env wiring of `run_http`, a real listener, and
//! interop with an independent MCP client implementation.

use std::net::TcpListener;
use std::process::Stdio;
use std::time::Duration;

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::{RoleClient, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::json;
use tokio::process::{Child, Command};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"e2e","version":"0"}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

/// A running `engramo-mcp http` child process. Killed on drop.
struct McpServer {
    base: String,
    _child: Child,
}

impl McpServer {
    /// Starts the binary against `api` and waits until it serves requests.
    async fn start(api: &MockServer) -> Self {
        Self::start_with_env(api, &[]).await
    }

    async fn start_with_env(api: &MockServer, extra_env: &[(&str, &str)]) -> Self {
        // A port reserved here and released for the child can be grabbed by a parallel
        // test in between; the child then exits on bind, and we retry with a fresh port.
        for _ in 0..5 {
            if let Some(server) = Self::try_start(api, extra_env).await {
                return server;
            }
        }
        panic!("engramo-mcp failed to start after 5 attempts");
    }

    /// `None` if the child exited before becoming ready (e.g. its port was taken).
    async fn try_start(api: &MockServer, extra_env: &[(&str, &str)]) -> Option<Self> {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let base = format!("http://127.0.0.1:{port}");

        // `env_clear` keeps the developer's/CI's own ENGRAMO_* and proxy variables out.
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_engramo-mcp"));
        cmd.arg("http")
            .env_clear()
            .env("ENGRAMO_API_URL", api.uri())
            .env("MCP_PUBLIC_URL", &base)
            .env("MCP_BIND_ADDR", format!("127.0.0.1:{port}"))
            .env("APP_ENV", "local")
            // Server logs are off by default (several tests provoke expected errors);
            // set E2E_RUST_LOG (e.g. `debug`) to see them when a test fails.
            .env(
                "RUST_LOG",
                std::env::var("E2E_RUST_LOG").unwrap_or_else(|_| "off".into()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if let Ok(root) = std::env::var("SystemRoot") {
            // Windows needs it for socket initialization.
            cmd.env("SystemRoot", root);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn engramo-mcp");

        // Probe a route that echoes this test's own mock API URL, so a ready answer can't
        // come from another test's server that happens to hold the same port.
        let probe = format!("{base}/.well-known/oauth-protected-resource");
        let http = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(r) = http.get(&probe).send().await
                && let Ok(meta) = r.json::<serde_json::Value>().await
                && meta["authorization_servers"][0] == api.uri()
            {
                return Some(Self {
                    base,
                    _child: child,
                });
            }
            if let Ok(Some(_)) = child.try_wait() {
                return None;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "engramo-mcp did not become ready at {base}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Opens an MCP session with rmcp's client, authenticated as `token`.
    async fn connect(&self, token: &str) -> Result<RunningService<RoleClient, ()>, String> {
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(self.base.clone()).auth_header(token),
        );
        ().serve(transport).await.map_err(|e| e.to_string())
    }

    /// Raw JSON-RPC POST to `/`, for assertions on status codes and headers.
    async fn post(
        &self,
        token: Option<&str>,
        session: Option<&str>,
        body: &str,
    ) -> reqwest::Response {
        let mut req = reqwest::Client::new()
            .post(&self.base)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body.to_string());
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(s) = session {
            req = req.header("mcp-session-id", s);
        }
        req.send().await.unwrap()
    }

    fn resource_metadata_url(&self) -> String {
        format!("{}/.well-known/oauth-protected-resource", self.base)
    }
}

async fn mock_me(api: &MockServer, status: u16) {
    Mock::given(method("GET"))
        .and(path("/me"))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_json(json!({"user_id": "11111111-2222-3333-4444-555555555555"})),
        )
        .mount(api)
        .await;
}

/// `GET /catalogs` returning one catalog named `name`, only for requests carrying `token`.
async fn mock_catalogs_for(api: &MockServer, token: &str, name: &str) {
    Mock::given(method("GET"))
        .and(path("/catalogs"))
        .and(header("x-api-key", token))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{
                "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "name": name,
                "version": 1
            }],
            "nextCursor": null
        })))
        .mount(api)
        .await;
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    tool: &'static str,
    args: serde_json::Value,
) -> CallToolResult {
    let params = CallToolRequestParams::new(tool)
        .with_arguments(args.as_object().cloned().unwrap_or_default());
    client.call_tool(params).await.expect("tools/call")
}

#[tokio::test]
async fn full_session_lists_tools_and_calls_one_with_the_session_token() {
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    mock_catalogs_for(&api, "engramo_alice", "Alice's Spanish").await;
    // A Gemini key in the environment must not surface the stdio-only TTS tools.
    let server = McpServer::start_with_env(
        &api,
        &[("ENGRAMO_TTS_GEMINI_API_KEYS", "k-should-be-ignored")],
    )
    .await;

    let client = server.connect("engramo_alice").await.expect("initialize");
    let info = client.peer_info().expect("server info");
    let name = info.server_info.as_ref().map(|i| i.name.as_str());
    assert_eq!(name, Some("engramo-mcp"));

    let tools = client.list_all_tools().await.expect("tools/list");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in ["list_catalogs", "get_server_version", "get_card"] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
    for absent in ["list_tts_voices", "generate_card_audio"] {
        assert!(!names.contains(&absent), "{absent} leaked into http mode");
    }

    let result = call(&client, "list_catalogs", json!({})).await;
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert!(
        text_of(&result).contains("Alice's Spanish"),
        "{}",
        text_of(&result)
    );

    let version = call(&client, "get_server_version", json!({})).await;
    assert!(
        text_of(&version).contains(env!("CARGO_PKG_VERSION")),
        "{}",
        text_of(&version)
    );

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn upstream_failure_is_a_tool_error_not_a_protocol_error() {
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    Mock::given(path("/catalogs"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&api)
        .await;
    let server = McpServer::start(&api).await;

    let client = server.connect("engramo_alice").await.expect("initialize");
    let result = call(&client, "list_catalogs", json!({})).await;
    assert_eq!(result.is_error, Some(true), "{}", text_of(&result));

    // The session survives the failed call.
    client
        .list_all_tools()
        .await
        .expect("tools/list after error");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn concurrent_sessions_each_act_as_their_own_user() {
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    mock_catalogs_for(&api, "engramo_alice", "Alice's Spanish").await;
    mock_catalogs_for(&api, "engramo_bob", "Bob's German").await;
    let server = McpServer::start(&api).await;

    let (alice, bob) = tokio::join!(
        server.connect("engramo_alice"),
        server.connect("engramo_bob")
    );
    let (alice, bob) = (alice.expect("alice"), bob.expect("bob"));

    let (a, b) = tokio::join!(
        call(&alice, "list_catalogs", json!({})),
        call(&bob, "list_catalogs", json!({}))
    );
    let (a, b) = (text_of(&a), text_of(&b));
    assert!(
        a.contains("Alice's Spanish") && !a.contains("Bob's German"),
        "{a}"
    );
    assert!(
        b.contains("Bob's German") && !b.contains("Alice's Spanish"),
        "{b}"
    );

    alice.cancel().await.unwrap();
    bob.cancel().await.unwrap();
}

#[tokio::test]
async fn missing_bearer_is_401_with_resource_metadata_challenge() {
    let api = MockServer::start().await;
    let server = McpServer::start(&api).await;

    let resp = server.post(None, None, INITIALIZE).await;
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()["www-authenticate"].to_str().unwrap(),
        format!(
            "Bearer resource_metadata=\"{}\"",
            server.resource_metadata_url()
        )
    );
}

#[tokio::test]
async fn malformed_token_is_rejected_without_an_upstream_call() {
    let api = MockServer::start().await;
    Mock::given(path("/me"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&api)
        .await;
    let server = McpServer::start(&api).await;

    let resp = server
        .post(Some("not-an-engramo-token"), None, INITIALIZE)
        .await;
    assert_eq!(resp.status(), 401);
    let challenge = resp.headers()["www-authenticate"].to_str().unwrap();
    assert!(challenge.contains("error=\"invalid_token\""), "{challenge}");

    assert!(server.connect("not-an-engramo-token").await.is_err());
}

#[tokio::test]
async fn token_rejected_by_api_is_401_invalid_token() {
    let api = MockServer::start().await;
    mock_me(&api, 401).await;
    let server = McpServer::start(&api).await;

    let resp = server.post(Some("engramo_revoked"), None, INITIALIZE).await;
    assert_eq!(resp.status(), 401);
    assert_eq!(
        resp.headers()["www-authenticate"].to_str().unwrap(),
        format!(
            "Bearer error=\"invalid_token\", resource_metadata=\"{}\"",
            server.resource_metadata_url()
        )
    );

    assert!(server.connect("engramo_revoked").await.is_err());
}

#[tokio::test]
async fn api_outage_during_session_open_is_503_without_challenge() {
    let api = MockServer::start().await;
    mock_me(&api, 503).await;
    let server = McpServer::start(&api).await;

    let resp = server.post(Some("engramo_alice"), None, INITIALIZE).await;
    assert_eq!(resp.status(), 503);
    assert!(!resp.headers().contains_key("www-authenticate"));
}

#[tokio::test]
async fn session_id_replayed_with_another_token_is_404() {
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    let server = McpServer::start(&api).await;

    let resp = server.post(Some("engramo_owner"), None, INITIALIZE).await;
    assert_eq!(resp.status(), 200);
    let sid = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    drop(resp);

    let resp = server
        .post(Some("engramo_attacker"), Some(&sid), INITIALIZED)
        .await;
    assert_eq!(resp.status(), 404);
    assert!(!resp.headers().contains_key("www-authenticate"));

    let resp = server
        .post(Some("engramo_owner"), Some(&sid), INITIALIZED)
        .await;
    assert_eq!(resp.status(), 202);
}

#[tokio::test]
async fn sessionless_non_initialize_post_is_400() {
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    let server = McpServer::start(&api).await;

    let resp = server
        .post(
            Some("engramo_alice"),
            None,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        )
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn unauthenticated_routes_are_served_without_a_token() {
    let api = MockServer::start().await;
    let server = McpServer::start(&api).await;
    let http = reqwest::Client::new();

    let version: serde_json::Value = http
        .get(format!("{}/version", server.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(version["version"], env!("CARGO_PKG_VERSION"), "{version}");

    let meta = http
        .get(server.resource_metadata_url())
        .send()
        .await
        .unwrap();
    assert_eq!(meta.status(), 200);
    let meta: serde_json::Value = meta.json().await.unwrap();
    assert_eq!(meta["resource"], server.base, "{meta}");
    assert_eq!(meta["authorization_servers"][0], api.uri(), "{meta}");
}
