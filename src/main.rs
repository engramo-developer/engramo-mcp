use std::sync::Arc;

use axum::middleware;
use clap::{Parser, Subcommand};
use engramo_mcp::{
    client::{EngramoClient, HardenedClient},
    config::McpConfig,
    http_auth::{
        AuthState, MAX_BODY_BYTES, SESSION_KEEP_ALIVE, SessionTokens, bearer_auth_middleware,
        current_bearer_token,
    },
    server::{EngramoMcpServer, build_session_server},
    tts, well_known,
};
use rmcp::{
    ServiceExt,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Parser)]
#[command(name = "engramo-mcp", version, about = "EngrAmo MCP server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run over stdio (default, for Claude Desktop / Cursor). One process serves one
    /// user, authenticated via the ENGRAMO_API_TOKEN env var.
    Stdio,
    /// Run over Streamable HTTP at `/` (for remote clients such as ChatGPT). Each
    /// session authenticates with its own `Authorization: Bearer <token>` header —
    /// no global ENGRAMO_API_TOKEN is used. Binds `MCP_BIND_ADDR` (default `0.0.0.0:8080`).
    Http,
}

/// Verbosity used when `RUST_LOG` is unset: `info` everywhere, except rmcp's session
/// manager, which logs every new `Mcp-Session-Id` at INFO (`create new session`). A
/// session id authorizes requests against the session's EngrAmo token, so it is a
/// credential-equivalent value and must not be shipped to Cloud Logging — see
/// `engramo_mcp::http_auth`.
const DEFAULT_LOG_FILTER: &str = "info,rmcp::transport::streamable_http_server::session=warn";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse(); // handles --version / --help before touching env vars

    init_logging();

    match cli.command {
        None | Some(Command::Stdio) => run_stdio().await,
        Some(Command::Http) => run_http().await,
    }
}

/// Sets up the global tracing subscriber. `APP_ENV=local` (or unset — the default for a
/// developer running `cargo run`/the stdio binary directly) gets human-readable output;
/// any other value (e.g. `dev`/`prod`, set by `cloudbuild.yaml` on real deployments) gets
/// GCP Cloud Logging-shaped structured JSON on stdout plus a second, Error-Reporting-shaped
/// JSON line on stderr for every `ERROR`-level event (see `error_reporting::ErrorReportingLayer`).
/// `RUST_LOG` still controls verbosity in both branches; unlike `EnvFilter`'s own default
/// (`ERROR` only), an unset `RUST_LOG` here defaults to [`DEFAULT_LOG_FILTER`] so routine
/// startup/operational logs aren't silently dropped on a fresh deployment that hasn't set it.
fn init_logging() {
    let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "local".to_string());
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));

    if app_env == "local" {
        Registry::default()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
    } else {
        let subscriber = Registry::default()
            .with(env_filter)
            .with(tracing_stackdriver::layer())
            .with(engramo_mcp::error_reporting::ErrorReportingLayer::new(
                "engramo-mcp",
                env!("CARGO_PKG_VERSION"),
            ));
        tracing::subscriber::set_global_default(subscriber).expect("Failed to set subscriber");

        std::panic::set_hook(Box::new(|panic_info| {
            let payload = panic_info.payload();
            let message: &str = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("Unknown panic");

            let location = panic_info
                .location()
                .map(|l| {
                    let file = std::path::Path::new(l.file())
                        .file_name()
                        .and_then(|f| f.to_str())
                        .unwrap_or(l.file());
                    format!("{file}:{}", l.line())
                })
                .unwrap_or_default();

            tracing::error!("Panic occurred at {}: {}", location, message);
        }));
    }
}

/// One process = one user. Reads `ENGRAMO_API_TOKEN` from the environment and holds a
/// single `EngramoClient` for the lifetime of the stdio connection (Claude Desktop, Cursor).
///
/// Also the **only** place [`tts::from_env`] is ever called — see the `tts` module's doc
/// comment for why that's a structural, grep-provable guarantee that a Gemini key can never
/// reach `http` mode.
async fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = McpConfig::from_env().map_err(config_help)?;
    let token = cfg.require_token().map_err(config_help)?;

    let client = EngramoClient::new(&cfg.api_url, token);
    let server = EngramoMcpServer::new(client, cfg.paid_ai_enabled);
    let server = attach_tts(server, tts::from_env())?;

    tracing::info!("Starting Engramo MCP server over stdio");

    let running = server.serve_with_ct(stdio(), Default::default()).await?;
    running.waiting().await?;

    Ok(())
}

fn config_help(e: engramo_mcp::config::ConfigError) -> String {
    format!(
        "{e}\n\nSet the required environment variables before running:\n  \
         ENGRAMO_API_URL=https://api.engramo.app\n  \
         ENGRAMO_API_TOKEN=<your-token>\n\n\
         Generate a token from your EngrAmo account settings."
    )
}

/// Wires up local TTS (`list_tts_voices`, `generate_card_audio`) onto a stdio `server` from
/// the result of [`tts::from_env`]. Pulled out of `run_stdio` (a pure move, no behavior
/// change) so the three arms — enabled, disabled, misconfigured — are directly testable.
fn attach_tts(
    server: EngramoMcpServer,
    tts_cfg: Result<Option<tts::TtsConfig>, tts::TtsConfigError>,
) -> Result<EngramoMcpServer, String> {
    match tts_cfg {
        Ok(Some(tts_cfg)) => {
            let provider = tts_cfg.provider.name();
            let model = tts_cfg.model.clone();
            let default_voice = tts_cfg.default_voice.clone();
            let key_count = tts_cfg.keys.len();
            // `build_engine` builds its own hardened client (a longer request timeout than
            // the main EngramoClient's, since Gemini synthesis is slower than a typical
            // EngrAmo API call) — see its doc comment for why callers can no longer supply
            // one themselves.
            let engine = tts::build_engine(tts_cfg);
            let server = server.with_tts(engine);
            tracing::info!(
                provider,
                model = %model,
                default_voice = %default_voice,
                key_count,
                "Local TTS enabled (generate_card_audio, list_tts_voices)"
            );
            Ok(server)
        }
        Ok(None) => {
            tracing::debug!("TTS disabled ({} not set)", tts::ENV_KEYS);
            Ok(server)
        }
        Err(e) => {
            // A misconfigured TTS setup (bad provider/model/voice) should be loud, not
            // silently disable the feature — the user explicitly opted in by setting
            // ENGRAMO_TTS_GEMINI_API_KEYS, so a typo elsewhere in their TTS config deserves
            // the same startup-failure treatment as a bad ENGRAMO_API_TOKEN.
            Err(format!(
                "{e}\n\nCheck your TTS environment variables ({}, {}, {}, {}).",
                tts::ENV_KEYS,
                tts::ENV_PROVIDER,
                tts::ENV_MODEL,
                tts::ENV_VOICE
            ))
        }
    }
}

/// Remote entry point: serves the MCP over Streamable HTTP at `/`, deriving a fresh
/// `EngramoClient` per session from the caller's own `Authorization: Bearer <token>` —
/// there is no global `ENGRAMO_API_TOKEN` in this mode (`McpConfig.api_token` is unused).
async fn run_http() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = McpConfig::from_env()
        .map_err(|e| format!("{e}\n\nSet ENGRAMO_API_URL before running `engramo-mcp http`."))?;
    let public_url = cfg.require_public_url().map_err(|e| {
        format!(
            "{e}\n\nSet MCP_PUBLIC_URL to this server's public URL (e.g. https://mcp.engramo.app) \
             before running `engramo-mcp http`: it is advertised in OAuth metadata and \
             WWW-Authenticate challenges."
        )
    })?;
    let public_url = public_url.to_string();

    // Local TTS is stdio-only (see `tts` module doc comment) — this reads only the env var's
    // *name*, never its value, and never builds a TtsConfig/engine. This is the only place
    // `http` mode references the `tts` module at all.
    if std::env::var_os(tts::ENV_KEYS).is_some() {
        tracing::warn!(
            "{} is set but is ignored in http mode — local TTS only runs over stdio, since \
             the key must never leave the user's machine. Run `engramo-mcp stdio` instead if \
             you want local TTS.",
            tts::ENV_KEYS
        );
    }

    // Shared across every session's `EngramoClient` (see `EngramoClient::with_http`) so
    // concurrent users reuse one connection pool instead of each paying for its own TLS
    // handshakes. Built by `EngramoClient::build_http_client` (not by hand here) so the
    // no-redirect policy that keeps a session's bearer token from following a 3xx to
    // another host can't be dropped by a future edit to this function.
    let http = EngramoClient::build_http_client();
    let app = build_app(&cfg, &public_url, http)?;

    let bind_addr = std::env::var("MCP_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(
        addr = %bind_addr,
        "Starting Engramo MCP server over Streamable HTTP at / — this binds plain HTTP; \
         bearer tokens are only protected in transit if a TLS-terminating proxy (e.g. \
         Cloud Run) sits in front of this listener"
    );

    axum::serve(listener, app).await?;
    Ok(())
}

/// rmcp's session manager with its idle timeout pinned to [`SESSION_KEEP_ALIVE`], the
/// constant the session-binding table is derived from.
fn session_manager() -> LocalSessionManager {
    let mut manager = LocalSessionManager::default();
    manager.session_config.keep_alive = Some(SESSION_KEEP_ALIVE);
    // Must match `StreamableHttpServerConfig::with_sse_retry(None)` in `build_app`:
    // the session layer primes every post-initialize request-wise SSE stream on its own.
    manager.session_config.sse_retry = None;
    manager
}

/// Builds the `http` mode axum `Router`: the auth-guarded MCP endpoint at `/` (bearer-auth
/// middleware + `MAX_BODY_BYTES` request body limit + rmcp's `StreamableHttpService`), plus
/// the unauthenticated `/version` and `/.well-known/oauth-protected-resource` routes. Kept
/// separate from `run_http` so tests can drive the router directly with
/// `tower::ServiceExt::oneshot` instead of binding a real listener.
///
/// `public_url` is the validated `MCP_PUBLIC_URL` (`McpConfig::require_public_url`). Fails
/// when it cannot be embedded in a `WWW-Authenticate` challenge.
fn build_app(
    cfg: &McpConfig,
    public_url: &str,
    http: HardenedClient,
) -> Result<axum::Router, String> {
    let public_url = public_url.to_string();
    let resource_metadata_url = format!(
        "{}/.well-known/oauth-protected-resource",
        public_url.trim_end_matches('/')
    );
    let api_url = cfg.api_url.clone();
    let paid_ai_enabled = cfg.paid_ai_enabled;
    let allowed_hosts = cfg.allowed_hosts();
    let auth_http = http.clone();
    let auth_api_url = api_url.clone();
    let factory = move || {
        let token = current_bearer_token().ok_or_else(|| {
            // Invariant violation: `bearer_auth_middleware` already rejects any
            // request without a non-empty bearer token before this factory ever
            // runs, so this should be unreachable. Log at ERROR (not a routine
            // auth failure) so it reaches GCP Error Reporting if it ever does fire.
            tracing::error!(
                "missing bearer token for this MCP session (auth middleware invariant violated)"
            );
            std::io::Error::other("missing bearer token for this MCP session")
        })?;
        Ok(build_session_server(
            &http,
            &api_url,
            &token,
            paid_ai_enabled,
        ))
    };

    // rmcp's `allowed_hosts` defaults to loopback-only (DNS-rebinding protection for
    // locally run servers) — a real deployment must extend it to its own domain, or
    // every authenticated request gets rejected with 403 "Host header is not
    // allowed" before it ever reaches this server's own auth/routing logic. See
    // `McpConfig::allowed_hosts`.
    tracing::info!(
        ?allowed_hosts,
        "Configured Host-header allowlist for http mode"
    );
    // Pin rmcp's idle timeout to the constant the session-binding table is derived from.
    let session_manager = Arc::new(session_manager());
    let service = StreamableHttpService::new(
        factory,
        session_manager.clone(),
        StreamableHttpServerConfig::default()
            .with_allowed_hosts(allowed_hosts)
            // rmcp's default prepends an SSE priming event (`id: 0`, `retry: 3000`,
            // empty `data:`) to every stream. Strict SSE parsers (Gemini) reject the
            // `retry:` line and drop the connection, and without an event store the
            // event enables no resumption anyway. This covers the `initialize` response
            // and the standalone GET stream; post-initialize request streams are primed
            // by the session layer, disabled in `session_manager()`.
            .with_sse_retry(None),
    );

    // Bearer auth applies only to the MCP endpoint itself (served at root, `/`) —
    // `.well-known/oauth-protected-resource` (RFC 9728, Track 3 Phase 3) must be
    // fetchable *without* a token, since its whole purpose is telling an
    // unauthenticated client where to go get one.
    let auth = AuthState::new(
        SessionTokens::new(),
        resource_metadata_url,
        auth_http,
        auth_api_url,
    )?
    .with_session_manager(session_manager);
    let mcp_router = axum::Router::new()
        .fallback_service(service)
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            MAX_BODY_BYTES,
        ))
        .layer(middleware::from_fn_with_state(
            auth.clone(),
            bearer_auth_middleware,
        ))
        .with_state(auth);

    let protected_resource_state = well_known::ProtectedResourceState {
        resource: public_url,
        authorization_server: cfg.api_url.clone(),
    };
    Ok(axum::Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            axum::routing::get(well_known::protected_resource_metadata),
        )
        .with_state(protected_resource_state)
        // OpenAI domain verification — unauthenticated, outside the bearer middleware.
        .route(
            "/.well-known/openai-apps-challenge",
            axum::routing::get(well_known::openai_apps_challenge)
                .with_state(cfg.openai_apps_challenge.clone()),
        )
        // Unauthenticated by design — it returns only this deployment's public build
        // version (name + version), nothing user- or token-scoped. Lives on the outer
        // router so it sits outside `bearer_auth_middleware`, like the `.well-known` route.
        .route(
            "/version",
            axum::routing::get(engramo_mcp::version::version_endpoint),
        )
        .merge(mcp_router))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn test_session_manager_pins_keep_alive() {
        assert_eq!(
            session_manager().session_config.keep_alive,
            Some(SESSION_KEEP_ALIVE)
        );
        assert!(session_manager().session_config.sse_retry.is_none());
    }

    fn test_app(cfg: &McpConfig) -> axum::Router {
        let url = cfg.require_public_url().unwrap().to_string();
        build_app(cfg, &url, EngramoClient::build_http_client()).unwrap()
    }

    #[test]
    fn test_build_app_rejects_unusable_public_url_instead_of_falling_back() {
        // `require_public_url` (what `run_http` calls) errors on an unset or unusable
        // value, and `build_app` itself refuses a URL that cannot be a header parameter.
        let unset = McpConfig::new("http://localhost", None, false).unwrap();
        assert!(unset.require_public_url().is_err());
        let err = build_app(
            &unset,
            "https://x.example/\"bad",
            EngramoClient::build_http_client(),
        )
        .unwrap_err();
        assert!(err.contains("quote"), "{err}");
    }

    fn test_cfg() -> McpConfig {
        McpConfig::new("http://localhost", None, false)
            .unwrap()
            .with_public_url("http://localhost:8080")
    }

    #[tokio::test]
    async fn test_http_app_rejects_body_over_max_body_bytes() {
        // Session-less POSTs are capped at MAX_INITIALIZE_BYTES by the auth middleware
        // (covered in http_auth.rs). This pins the outer MAX_BODY_BYTES limit that guards
        // session-bearing POSTs, which rmcp buffers in full.
        let api = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/me"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"user_id": "11111111-2222-3333-4444-555555555555"}),
            ))
            .mount(&api)
            .await;
        let cfg = McpConfig::new(api.uri(), None, false)
            .unwrap()
            .with_public_url("http://localhost:8080");
        let app = test_app(&cfg);
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let req = Request::post("/")
            .header("authorization", "Bearer engramo_t")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("host", "localhost")
            .body(Body::from(init))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let sid = resp.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_string();

        let body = vec![b'a'; MAX_BODY_BYTES + 1];
        let req = Request::post("/")
            .header("authorization", "Bearer engramo_t")
            .header("mcp-session-id", sid)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("host", "localhost")
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 10_000)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            status == StatusCode::PAYLOAD_TOO_LARGE
                || (status == StatusCode::INTERNAL_SERVER_ERROR
                    && text.contains("length limit exceeded")),
            "{status} {text}"
        );
    }

    #[tokio::test]
    async fn test_http_app_sse_stream_has_no_priming_event() {
        // Gemini's MCP client fails with "Invalid SSE response ... Line: retry: 3000"
        // on rmcp's default priming event; the first SSE event must be the JSON-RPC result.
        let api = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/me"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"user_id": "11111111-2222-3333-4444-555555555555"}),
            ))
            .mount(&api)
            .await;
        let cfg = McpConfig::new(api.uri(), None, false)
            .unwrap()
            .with_public_url("http://localhost:8080");
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let req = Request::post("/")
            .header("authorization", "Bearer engramo_t")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("host", "localhost")
            .body(Body::from(init))
            .unwrap();
        let resp = test_app(&cfg).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            axum::body::to_bytes(resp.into_body(), 1_000_000),
        )
        .await
        .expect("SSE response body did not end")
        .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.starts_with(r#"data: {"jsonrpc":"2.0","id":1,"result":"#),
            "{text}"
        );
        assert!(
            !text
                .lines()
                .any(|l| l.starts_with("retry:") || l.starts_with("id:")),
            "{text}"
        );
    }

    /// Opens a session on `app` (initialize + notifications/initialized) and returns its id.
    async fn open_session(app: &axum::Router) -> String {
        let post = |session: Option<&str>, body: &'static str| {
            let mut b = Request::post("/")
                .header("authorization", "Bearer engramo_t")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("host", "localhost");
            if let Some(s) = session {
                b = b.header("mcp-session-id", s);
            }
            b.body(Body::from(body)).unwrap()
        };
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let resp = app.clone().oneshot(post(None, init)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let sid = resp.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_string();
        let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let resp = app
            .clone()
            .oneshot(post(Some(&sid), initialized))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        sid
    }

    async fn mock_me() -> wiremock::MockServer {
        let api = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/me"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"user_id": "11111111-2222-3333-4444-555555555555"}),
            ))
            .mount(&api)
            .await;
        api
    }

    #[tokio::test]
    async fn test_http_app_post_initialize_sse_stream_has_no_retry_line() {
        // Follow-up requests are primed by the session layer, not the tower layer.
        let api = mock_me().await;
        let cfg = McpConfig::new(api.uri(), None, false)
            .unwrap()
            .with_public_url("http://localhost:8080");
        let app = test_app(&cfg);
        let sid = open_session(&app).await;
        let req = Request::post("/")
            .header("authorization", "Bearer engramo_t")
            .header("mcp-session-id", sid)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("host", "localhost")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            ))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            axum::body::to_bytes(resp.into_body(), 1_000_000),
        )
        .await
        .expect("SSE response body did not end")
        .unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.starts_with(r#"data: {"jsonrpc":"2.0","id":2,"result":"#),
            "{text}"
        );
        assert!(!text.lines().any(|l| l.starts_with("retry:")), "{text}");
    }

    #[tokio::test]
    async fn test_http_app_standalone_get_stream_has_no_priming_event() {
        use http_body_util::BodyExt;
        let api = mock_me().await;
        let cfg = McpConfig::new(api.uri(), None, false)
            .unwrap()
            .with_public_url("http://localhost:8080");
        let app = test_app(&cfg);
        let sid = open_session(&app).await;
        let req = Request::get("/")
            .header("authorization", "Bearer engramo_t")
            .header("mcp-session-id", sid)
            .header("accept", "text/event-stream")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        let mut body = resp.into_body();
        // The stream stays open: read only the first frame, if any.
        match tokio::time::timeout(std::time::Duration::from_millis(500), body.frame()).await {
            Err(_) => {} // no frame emitted, so no priming event
            Ok(Some(Ok(f))) => {
                if let Ok(d) = f.into_data() {
                    let t = String::from_utf8_lossy(&d);
                    assert!(
                        !t.lines()
                            .any(|l| l.starts_with("retry:") || l.starts_with("id:")),
                        "{t}"
                    );
                }
            }
            Ok(other) => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_http_app_session_is_bound_to_the_opening_token() {
        let api = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/me"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"user_id": "11111111-2222-3333-4444-555555555555"}),
            ))
            .mount(&api)
            .await;
        let cfg = McpConfig::new(api.uri(), None, false)
            .unwrap()
            .with_public_url("http://localhost:8080");
        let app = test_app(&cfg);
        let post = |token: &str, session: Option<&str>, body: &'static str| {
            let mut b = Request::post("/")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("host", "localhost");
            if let Some(s) = session {
                b = b.header("mcp-session-id", s);
            }
            b.body(Body::from(body)).unwrap()
        };
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

        let resp = app
            .clone()
            .oneshot(post("engramo_owner", None, init))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let sid = resp.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_string();

        let resp = app
            .clone()
            .oneshot(post("engramo_attacker", Some(&sid), initialized))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(!resp.headers().contains_key("www-authenticate"));
        let bytes = axum::body::to_bytes(resp.into_body(), 10_000)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"Not Found: Session not found");

        let resp = app
            .oneshot(post("engramo_owner", Some(&sid), initialized))
            .await
            .unwrap();
        assert_ne!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_http_app_version_is_unauthenticated_but_root_requires_bearer() {
        let app = test_app(&test_cfg());
        let v = app
            .clone()
            .oneshot(
                Request::get("/version")
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(v.status(), StatusCode::OK);

        let root = app
            .oneshot(
                Request::post("/")
                    .header("host", "localhost")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(root.status(), StatusCode::UNAUTHORIZED);
    }

    async fn get_status_and_body(
        app: axum::Router,
        uri: &str,
    ) -> (StatusCode, String, Option<String>) {
        let resp = app
            .oneshot(
                Request::get(uri)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let ct = resp
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_string());
        let bytes = axum::body::to_bytes(resp.into_body(), 10_000)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string(), ct)
    }

    #[tokio::test]
    async fn test_openai_challenge_served_when_set() {
        let cfg = test_cfg().with_openai_apps_challenge("tok-xyz");
        let app = test_app(&cfg);
        let (status, body, ct) =
            get_status_and_body(app, "/.well-known/openai-apps-challenge").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "tok-xyz");
        assert!(ct.unwrap().starts_with("text/plain"));
    }

    #[tokio::test]
    async fn test_openai_challenge_404_when_unset() {
        let app = test_app(&test_cfg());
        let (status, _, _) = get_status_and_body(app, "/.well-known/openai-apps-challenge").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_root_401_advertises_metadata_url_with_single_slash() {
        let cfg = McpConfig::new("http://localhost", None, false)
            .unwrap()
            .with_public_url("https://mcp.engramo.app/");
        let app = test_app(&cfg);
        let resp = app
            .oneshot(
                Request::post("/")
                    .header("host", "localhost")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get("www-authenticate").unwrap(),
            "Bearer resource_metadata=\"https://mcp.engramo.app/.well-known/oauth-protected-resource\""
        );
    }

    fn test_server() -> EngramoMcpServer {
        let client = EngramoClient::new("http://localhost", "engramo_test_token");
        EngramoMcpServer::new(client, false)
    }

    #[test]
    fn test_attach_tts_none_leaves_server_without_tts_tools() {
        use rmcp::ServerHandler;

        let server = attach_tts(test_server(), Ok(None)).unwrap();
        let instructions = server.get_info().instructions.unwrap_or_default();
        assert!(
            !instructions.contains("generate_card_audio"),
            "{instructions}"
        );
    }

    #[test]
    fn test_attach_tts_some_registers_engine() {
        use engramo_mcp::config::Redacted;
        use rmcp::ServerHandler;
        use tts::{TtsConfig, TtsProvider};

        let cfg = TtsConfig {
            provider: TtsProvider::Gemini,
            keys: vec![Redacted::new("k".to_string())],
            model: "m".to_string(),
            default_voice: "Puck".to_string(),
        };
        let server = attach_tts(test_server(), Ok(Some(cfg))).unwrap();
        let instructions = server.get_info().instructions.unwrap_or_default();
        assert!(
            instructions.contains("generate_card_audio"),
            "{instructions}"
        );
    }

    #[test]
    fn test_attach_tts_err_fails_startup_with_env_var_help() {
        let result = attach_tts(
            test_server(),
            Err(tts::TtsConfigError::UnknownProvider("x".to_string())),
        );
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected attach_tts to fail startup"),
        };
        assert!(err.contains(tts::ENV_KEYS), "{err}");
        assert!(err.contains(tts::ENV_PROVIDER), "{err}");
        assert!(err.contains(tts::ENV_MODEL), "{err}");
        assert!(err.contains(tts::ENV_VOICE), "{err}");
    }
}
