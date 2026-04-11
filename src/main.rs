use clap::{Parser, Subcommand};
use engram_mcp::{client::EngramClient, config::McpConfig, server::EngramMcpServer};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "engram-mcp", about = "Engram MCP server")]
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
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cfg = McpConfig::from_env()?;
    let client = EngramClient::new(&cfg.api_url, &cfg.api_token);
    let server = EngramMcpServer::new(client);

    let _ = Cli::parse(); // validates args; default is stdio
    tracing::info!("Starting Engram MCP server over stdio");

    let running = server.serve_with_ct(stdio(), Default::default()).await?;
    running.waiting().await?;

    Ok(())
}
