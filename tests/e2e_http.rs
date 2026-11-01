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

/// A running `engramo-mcp http` child process. Stopped on drop: with SIGTERM on Unix, so
/// the binary exits through its graceful-shutdown path (and an instrumented build writes
/// its coverage profile), with a hard kill elsewhere.
struct McpServer {
    base: String,
    child: Child,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.sigterm() {
            // Bounded reap: don't outlive the test binary or write the coverage profile late.
            for _ in 0..100 {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        let _ = self.child.start_kill();
    }
}

impl McpServer {
    /// Starts the binary against `api` and waits until it serves requests.
    async fn start(api: &MockServer) -> Self {
        Self::start_with_env(api, &[]).await
    }

    /// Like `start`, but leaves `RUST_LOG` unset (so the binary's default log filter
    /// applies) and pipes stdout/stderr so a test can inspect what was logged.
    #[cfg(unix)]
    async fn start_capturing_logs(api: &MockServer, extra_env: &[(&str, &str)]) -> Self {
        Self::start_inner(api, extra_env, true).await
    }

    async fn start_with_env(api: &MockServer, extra_env: &[(&str, &str)]) -> Self {
        Self::start_inner(api, extra_env, false).await
    }

    async fn start_inner(api: &MockServer, extra_env: &[(&str, &str)], capture_logs: bool) -> Self {
        // A port reserved here and released for the child can be grabbed by a parallel
        // test in between; the child then exits on bind, and we retry with a fresh port.
        for _ in 0..5 {
            if let Some(server) = Self::try_start(api, extra_env, capture_logs).await {
                return server;
            }
        }
        panic!("engramo-mcp failed to start after 5 attempts");
    }

    /// `None` if the child exited before becoming ready (e.g. its port was taken).
    async fn try_start(
        api: &MockServer,
        extra_env: &[(&str, &str)],
        capture_logs: bool,
    ) -> Option<Self> {
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
            .stdin(Stdio::null());
        if capture_logs {
            cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        } else {
            // Server logs are off by default (several tests provoke expected errors);
            // set E2E_RUST_LOG (e.g. `debug`) to see them when a test fails.
            cmd.env(
                "RUST_LOG",
                std::env::var("E2E_RUST_LOG").unwrap_or_else(|_| "off".into()),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        }
        if let Ok(root) = std::env::var("SystemRoot") {
            // Windows needs it for socket initialization.
            cmd.env("SystemRoot", root);
        }
        if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
            // Lets a coverage run (cargo-llvm-cov) collect the child's profile, which it
            // writes on the graceful exit SIGTERM triggers.
            cmd.env("LLVM_PROFILE_FILE", profile);
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn engramo-mcp");

        // Probe a route that echoes this test's own mock API URL, so a ready answer can't
        // come from another test's server that happens to hold the same port.
        let probe = format!("{base}/.well-known/oauth-protected-resource");
        let http = loopback_client();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(r) = http.get(&probe).send().await
                && let Ok(meta) = r.json::<serde_json::Value>().await
                && meta["authorization_servers"][0] == api.uri()
            {
                return Some(Self { base, child });
            }
            if let Ok(Some(_)) = child.try_wait() {
                return None;
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.start_kill();
                panic!("engramo-mcp did not become ready at {base}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Sends `SIG<sig>` (e.g. `"TERM"`) to the child; `false` if it could not be delivered.
    #[cfg(unix)]
    fn signal(&self, sig: &str) -> bool {
        let Some(pid) = self.child.id() else {
            return false;
        };
        std::process::Command::new("kill")
            .args([format!("-{sig}"), pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Sends SIGTERM to the child; `false` if it could not be delivered.
    #[cfg(unix)]
    fn sigterm(&self) -> bool {
        self.signal("TERM")
    }

    /// Waits for the child to exit, returning its status.
    #[cfg(unix)]
    async fn wait_exit(&mut self, within: Duration) -> std::process::ExitStatus {
        tokio::time::timeout(within, self.child.wait())
            .await
            .expect("engramo-mcp did not exit in time")
            .unwrap()
    }

    /// Sends SIGTERM and waits for the child to exit, returning its status.
    #[cfg(unix)]
    async fn terminate(&mut self, within: Duration) -> std::process::ExitStatus {
        assert!(self.sigterm(), "failed to send SIGTERM");
        self.wait_exit(within).await
    }

    /// Opens an MCP session with rmcp's client, authenticated as `token`.
    async fn connect(&self, token: &str) -> Result<RunningService<RoleClient, ()>, String> {
        let transport = StreamableHttpClientTransport::with_client(
            loopback_client(),
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
        let mut req = loopback_client()
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

/// A client that ignores `HTTP_PROXY`/`ALL_PROXY`, which would otherwise swallow loopback
/// requests. Otherwise like rmcp's default transport client: no idle pooling, and no
/// redirects followed, so a stray 3xx surfaces in status assertions.
fn loopback_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Runs `engramo-mcp http` with exactly `env` and returns (status, stderr). Fails the
/// test if the binary starts serving instead of exiting.
async fn run_to_exit(env: &[(&str, &str)]) -> (std::process::ExitStatus, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_engramo-mcp"));
    cmd.arg("http")
        .env_clear()
        .envs(env.iter().copied())
        .env("APP_ENV", "local")
        .env("RUST_LOG", "off")
        .env("MCP_BIND_ADDR", "127.0.0.1:0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Ok(root) = std::env::var("SystemRoot") {
        cmd.env("SystemRoot", root);
    }
    if let Ok(profile) = std::env::var("LLVM_PROFILE_FILE") {
        // Lets a coverage run collect the child's profile instead of the child writing
        // `default.profraw` into the package root.
        cmd.env("LLVM_PROFILE_FILE", profile);
    }
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .expect("engramo-mcp started instead of failing on missing config")
        .unwrap();
    (
        out.status,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
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
    let http = loopback_client();

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

#[cfg(unix)]
#[tokio::test]
async fn sigterm_exits_cleanly_when_idle() {
    let api = MockServer::start().await;
    let mut mcp = McpServer::start(&api).await;

    let status = mcp.terminate(Duration::from_secs(3)).await;
    assert!(status.success(), "exit status after SIGTERM: {status}");
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_exits_cleanly_with_an_open_session() {
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    let mut mcp = McpServer::start(&api).await;

    // Open the standalone SSE stream ourselves so it is known to be open: it never ends
    // on its own, so shutdown has to end it after the grace period rather than wait for
    // the platform's SIGKILL.
    let resp = mcp.post(Some("engramo_alice"), None, INITIALIZE).await;
    let sid = resp.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    drop(resp);
    assert_eq!(
        mcp.post(Some("engramo_alice"), Some(&sid), INITIALIZED)
            .await
            .status(),
        202
    );
    let stream = loopback_client()
        .get(&mcp.base)
        .bearer_auth("engramo_alice")
        .header("mcp-session-id", &sid)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), 200); // headers received => the stream is open server-side

    let started = tokio::time::Instant::now();
    let status = mcp.terminate(Duration::from_secs(9)).await;
    assert!(status.success(), "exit status after SIGTERM: {status}");
    let elapsed = started.elapsed();
    // >= SHUTDOWN_GRACE: the open stream held shutdown. Well under SHUTDOWN_GRACE +
    // SHUTDOWN_FORCE_AFTER (8 s): the SSE-stream cancel ended it, not the hard deadline,
    // which also exits 0 and would otherwise hide a broken `sse_shutdown` wiring.
    assert!(
        (Duration::from_secs(4)..Duration::from_secs(7)).contains(&elapsed),
        "{elapsed:?}"
    );
    drop(stream);
}

#[cfg(unix)]
#[tokio::test]
async fn sigint_exits_cleanly_when_idle() {
    let api = MockServer::start().await;
    let mut mcp = McpServer::start(&api).await;
    assert!(mcp.signal("INT"), "failed to send SIGINT");
    let status = mcp.wait_exit(Duration::from_secs(3)).await;
    assert!(status.success(), "exit status after SIGINT: {status}");
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_lets_an_in_flight_tool_call_finish_and_stops_accepting() {
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    Mock::given(method("GET"))
        .and(path("/catalogs"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(1500)) // < SHUTDOWN_GRACE (5 s)
                .set_body_json(json!({
                    "data": [{"id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "name": "Slow deck", "version": 1}],
                    "nextCursor": null
                })),
        )
        .mount(&api)
        .await;
    let mut mcp = McpServer::start(&api).await;
    let client = mcp.connect("engramo_alice").await.unwrap();

    let (result, ()) = tokio::join!(call(&client, "list_catalogs", json!({})), async {
        // SIGTERM only once the tool call is provably inside its handler (the mock API has
        // its upstream request), not after a fixed sleep a loaded runner can outrun.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !api
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| r.url.path() == "/catalogs")
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "tool call never reached the API"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(mcp.sigterm());
        // The in-flight call keeps the process alive, so a refused connect means the
        // listener was closed, not that the process exited.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Err(e) = loopback_client()
                .get(format!("{}/version", mcp.base))
                .send()
                .await
                && e.is_connect()
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "still accepting after SIGTERM"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert!(
        text_of(&result).contains("Slow deck"),
        "{}",
        text_of(&result)
    );
    let status = mcp.wait_exit(Duration::from_secs(9)).await;
    assert!(status.success(), "{status}");
    drop(client);
}

#[cfg(unix)]
#[tokio::test]
async fn default_log_filter_never_logs_a_session_id() {
    use tokio::io::AsyncReadExt;
    for app_env in ["local", "prod"] {
        let api = MockServer::start().await;
        mock_me(&api, 200).await;
        let mut mcp = McpServer::start_capturing_logs(&api, &[("APP_ENV", app_env)]).await;

        let resp = mcp.post(Some("engramo_alice"), None, INITIALIZE).await;
        assert_eq!(resp.status(), 200);
        let sid = resp.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_string();
        drop(resp);
        // A request on the session too, so the request path is exercised, not just creation.
        let resp = mcp
            .post(Some("engramo_alice"), Some(&sid), INITIALIZED)
            .await;
        assert_eq!(resp.status(), 202);

        assert!(mcp.terminate(Duration::from_secs(9)).await.success());
        let mut logs = String::new();
        mcp.child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut logs)
            .await
            .unwrap();
        mcp.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut logs)
            .await
            .unwrap();
        // Not vacuous: INFO logging really was on.
        assert!(
            logs.contains("Starting Engramo MCP server"),
            "{app_env}: no logs captured:\n{logs}"
        );
        assert!(
            !logs.contains(&sid),
            "{app_env}: session id leaked into logs:\n{logs}"
        );
    }
}

#[tokio::test]
async fn http_mode_refuses_to_start_without_required_config() {
    let (status, stderr) = run_to_exit(&[("ENGRAMO_API_URL", "http://127.0.0.1:1")]).await;
    assert!(!status.success());
    assert!(stderr.contains("MCP_PUBLIC_URL"), "{stderr}");

    let (status, stderr) = run_to_exit(&[("MCP_PUBLIC_URL", "http://127.0.0.1:1")]).await;
    assert!(!status.success());
    assert!(stderr.contains("ENGRAMO_API_URL"), "{stderr}");
}

#[tokio::test]
async fn resources_and_prompts_are_served_with_the_session_token() {
    use rmcp::model::{ErrorCode, GetPromptRequestParams, ReadResourceRequestParams};
    let api = MockServer::start().await;
    mock_me(&api, 200).await;
    mock_catalogs_for(&api, "engramo_alice", "Alice's Spanish").await;
    mock_catalogs_for(&api, "engramo_bob", "Bob's German").await;
    let server = McpServer::start(&api).await;
    let alice = server.connect("engramo_alice").await.unwrap();
    let bob = server.connect("engramo_bob").await.unwrap();

    let uris: Vec<String> = alice
        .list_all_resources()
        .await
        .unwrap()
        .iter()
        .map(|r| r.uri.clone())
        .collect();
    for u in ["engramo://catalogs", "engramo://card-schema"] {
        assert!(uris.iter().any(|x| x == u), "missing {u}: {uris:?}");
    }

    let a = serde_json::to_string(
        &alice
            .read_resource(ReadResourceRequestParams::new("engramo://catalogs"))
            .await
            .unwrap(),
    )
    .unwrap();
    let b = serde_json::to_string(
        &bob.read_resource(ReadResourceRequestParams::new("engramo://catalogs"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        a.contains("Alice's Spanish") && !a.contains("Bob's German"),
        "{a}"
    );
    assert!(
        b.contains("Bob's German") && !b.contains("Alice's Spanish"),
        "{b}"
    );

    let err = alice
        .read_resource(ReadResourceRequestParams::new("engramo://nope"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, rmcp::ServiceError::McpError(ref e) if e.code == ErrorCode::INVALID_PARAMS),
        "{err:?}"
    );

    let prompts: Vec<String> = alice
        .list_all_prompts()
        .await
        .unwrap()
        .iter()
        .map(|p| p.name.to_string())
        .collect();
    assert!(prompts.iter().any(|p| p == "review_session"), "{prompts:?}");
    let p = alice
        .get_prompt(GetPromptRequestParams::new("review_session"))
        .await
        .unwrap();
    assert!(!p.messages.is_empty());
    let err = alice
        .get_prompt(GetPromptRequestParams::new("nope"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, rmcp::ServiceError::McpError(ref e) if e.code == ErrorCode::INVALID_PARAMS),
        "{err:?}"
    );

    alice
        .list_all_tools()
        .await
        .expect("session survives protocol errors");
    alice.cancel().await.unwrap();
    bob.cancel().await.unwrap();
}
