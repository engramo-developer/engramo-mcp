use std::sync::Arc;

use axum::{
    extract::Request,
    http::{StatusCode, header::AUTHORIZATION},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use clap::{Parser, Subcommand};
use engramo_mcp::{client::EngramClient, config::McpConfig, server::EngramMcpServer, well_known};
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
    /// user, authenticated via the ENGRAM_API_TOKEN env var.
    Stdio,
    /// Run over Streamable HTTP at `/mcp` (for remote clients such as ChatGPT). Each
    /// session authenticates with its own `Authorization: Bearer <token>` header —
    /// no global ENGRAM_API_TOKEN is used. Binds `MCP_BIND_ADDR` (default `0.0.0.0:8080`).
    Http,
}

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
/// (`ERROR` only), an unset `RUST_LOG` here defaults to `info` so routine startup/operational
/// logs aren't silently dropped on a fresh deployment that hasn't set it.
fn init_logging() {
    let app_env = std::env::var("APP_ENV").unwrap_or_else(|_| "local".to_string());
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

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

/// One process = one user. Reads `ENGRAM_API_TOKEN` from the environment and holds a
/// single `EngramClient` for the lifetime of the stdio connection (Claude Desktop, Cursor).
async fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = McpConfig::from_env().map_err(config_help)?;
    let token = cfg.require_token().map_err(config_help)?;

    let client = EngramClient::new(&cfg.api_url, token);
    let server = EngramMcpServer::new(client, cfg.paid_ai_enabled);

    tracing::info!("Starting Engram MCP server over stdio");

    let running = server.serve_with_ct(stdio(), Default::default()).await?;
    running.waiting().await?;

    Ok(())
}

fn config_help(e: engramo_mcp::config::ConfigError) -> String {
    format!(
        "{e}\n\nSet the required environment variables before running:\n  \
         ENGRAM_API_URL=https://api.engramo.app\n  \
         ENGRAM_API_TOKEN=<your-token>\n\n\
         Generate a token from your EngrAmo account settings."
    )
}

// Carries the per-request bearer token from the axum auth middleware to the rmcp
// `StreamableHttpService` session factory, which is a plain `Fn() -> Result<S, io::Error>`
// with no access to the HTTP request. The factory is invoked synchronously, inline,
// while handling the `initialize` request that opens a new MCP session (see
// `StreamableHttpService::handle_post` in rmcp) — i.e. still inside the async task this
// task-local is scoped over — so `try_with` reliably sees the value the middleware set.
tokio::task_local! {
    static CURRENT_BEARER_TOKEN: String;
}

/// Remote entry point: serves the MCP over Streamable HTTP at `/mcp`, deriving a fresh
/// `EngramClient` per session from the caller's own `Authorization: Bearer <token>` —
/// there is no global `ENGRAM_API_TOKEN` in this mode (`McpConfig.api_token` is unused).
async fn run_http() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = McpConfig::from_env()
        .map_err(|e| format!("{e}\n\nSet ENGRAM_API_URL before running `engramo-mcp http`."))?;

    let api_url = cfg.api_url.clone();
    let paid_ai_enabled = cfg.paid_ai_enabled;
    let allowed_hosts = cfg.allowed_hosts();
    let factory = move || {
        let token = CURRENT_BEARER_TOKEN.try_with(|t| t.clone()).map_err(|_| {
            // Invariant violation: `bearer_auth_middleware` already rejects any
            // request without a non-empty bearer token before this factory ever
            // runs, so this should be unreachable. Log at ERROR (not a routine
            // auth failure) so it reaches GCP Error Reporting if it ever does fire.
            tracing::error!(
                "missing bearer token for this MCP session (auth middleware invariant violated)"
            );
            std::io::Error::other("missing bearer token for this MCP session")
        })?;
        let client = EngramClient::new(&api_url, &token);
        Ok(EngramMcpServer::new(client, paid_ai_enabled))
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
    let session_manager = Arc::new(LocalSessionManager::default());
    let service = StreamableHttpService::new(
        factory,
        session_manager,
        StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts),
    );

    // Bearer auth applies only to `/mcp` — `.well-known/oauth-protected-resource`
    // (RFC 9728, Track 3 Phase 3) must be fetchable *without* a token, since its
    // whole purpose is telling an unauthenticated client where to go get one.
    let mcp_router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(bearer_auth_middleware));

    let protected_resource_state = well_known::ProtectedResourceState {
        resource: cfg.public_url.clone().unwrap_or_else(|| {
            tracing::warn!(
                "MCP_PUBLIC_URL is not set — .well-known/oauth-protected-resource will \
                 advertise a placeholder \"resource\" value. Set MCP_PUBLIC_URL to this \
                 server's real public URL (e.g. https://mcp.engramo.app/mcp) before \
                 deploying for OAuth."
            );
            "http://localhost:8080/mcp".to_string()
        }),
        authorization_server: cfg.api_url.clone(),
    };
    let app = axum::Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            axum::routing::get(well_known::protected_resource_metadata),
        )
        .with_state(protected_resource_state)
        .merge(mcp_router);

    let bind_addr = std::env::var("MCP_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %bind_addr, "Starting Engram MCP server over Streamable HTTP at /mcp");

    axum::serve(listener, app).await?;
    Ok(())
}

/// Extracts `Authorization: Bearer <token>` and scopes it into `CURRENT_BEARER_TOKEN`
/// for the duration of the request. Missing or empty bearer tokens are rejected with
/// 401 at the edge — before ever reaching the rmcp session factory (which would 500 on
/// a missing task-local, since it has no HTTP-status-aware rejection path of its own).
async fn bearer_auth_middleware(req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string);

    match token {
        Some(token) => CURRENT_BEARER_TOKEN.scope(token, next.run(req)).await,
        None => (
            StatusCode::UNAUTHORIZED,
            "Missing or empty Authorization: Bearer <token> header",
        )
            .into_response(),
    }
}
