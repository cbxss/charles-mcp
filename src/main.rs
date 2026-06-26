//! charles-mcp — an MCP stdio server for Charles Proxy 5.

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

use charles_mcp::config::Config;
use charles_mcp::server::CharlesServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs MUST go to stderr — stdout is the MCP stdio transport.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("CHARLES_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let cfg = Arc::new(Config::parse());
    if cfg.web_pass.is_some() && cfg.web_user.is_none() {
        tracing::warn!(
            "--web-pass is set without --web-user; the password is ignored (basic auth needs both)"
        );
    }
    tracing::info!(proxy = %cfg.proxy_url(), "starting charles-mcp");

    let server = CharlesServer::new(cfg).context("initializing Charles server")?;
    let running = server
        .serve(stdio())
        .await
        .context("starting MCP stdio service")?;
    let reason = running.waiting().await?;
    tracing::info!(?reason, "charles-mcp stopped");
    Ok(())
}
