use clap::{Parser, Subcommand};
use engram_mcp::{client::EngramClient, config::McpConfig, server::EngramMcpServer};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "engram-mcp", version, about = "Engram MCP server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run over stdio (default, for Claude Desktop / Cursor)
    Stdio,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = Cli::parse(); // handles --version / --help before touching env vars

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cfg = McpConfig::from_env().map_err(|e| {
        format!(
            "{e}\n\nSet the required environment variables before running:\n  \
             ENGRAM_API_URL=https://api-engram.volmyr.com\n  \
             ENGRAM_API_TOKEN=<your-token>\n\n\
             Generate a token at https://engram.volmyr.com"
        )
    })?;

    let client = EngramClient::new(&cfg.api_url, &cfg.api_token);
    let server = EngramMcpServer::new(client);

    tracing::info!("Starting Engram MCP server over stdio");

    let running = server.serve_with_ct(stdio(), Default::default()).await?;
    running.waiting().await?;

    Ok(())
}
