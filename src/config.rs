//! Runtime configuration (CLI flags with environment-variable fallbacks).

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

/// Connection + behavior settings for the Charles MCP server.
///
/// Precedence is the clap default: an explicit CLI flag overrides the
/// environment variable, which overrides the built-in default.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "charles-mcp",
    version,
    about = "MCP server for Charles Proxy 5"
)]
pub struct Config {
    /// Host of the running Charles HTTP proxy.
    #[arg(long, env = "CHARLES_PROXY_HOST", default_value = "127.0.0.1")]
    pub proxy_host: String,

    /// Port of the running Charles HTTP proxy.
    #[arg(long, env = "CHARLES_PROXY_PORT", default_value_t = 8888)]
    pub proxy_port: u16,

    /// Magic host the Charles Web Interface answers on (reached *through* the proxy).
    #[arg(long, env = "CHARLES_CONTROL_HOST", default_value = "control.charles")]
    pub control_host: String,

    /// Username for Web Interface basic auth (if configured in Charles).
    #[arg(long, env = "CHARLES_WEB_USER")]
    pub web_user: Option<String>,

    /// Password for Web Interface basic auth (if configured in Charles).
    #[arg(long, env = "CHARLES_WEB_PASS")]
    pub web_pass: Option<String>,

    /// Path to the Charles binary, used for `charles convert` of `.chls` files.
    #[arg(
        long,
        env = "CHARLES_BIN",
        default_value = "/Applications/Charles.app/Contents/MacOS/Charles"
    )]
    pub charles_bin: PathBuf,

    /// Per-request timeout in milliseconds.
    #[arg(long, env = "CHARLES_TIMEOUT_MS", default_value_t = 15_000)]
    pub timeout_ms: u64,

    /// Default cap on decoded body bytes returned by `get_request`.
    #[arg(long, env = "CHARLES_BODY_MAX_BYTES", default_value_t = 8_192)]
    pub body_max_bytes: usize,

    /// Preferred format when fetching/exporting the live session.
    #[arg(long, env = "CHARLES_EXPORT_FORMAT", default_value = "chlsj")]
    pub default_export_format: String,
}

impl Config {
    /// `http://host:port` URL of the Charles proxy.
    pub fn proxy_url(&self) -> String {
        format!("http://{}:{}", self.proxy_host, self.proxy_port)
    }

    /// Build a full `http://control.charles/<path>` URL (path may start with `/`).
    pub fn control_url(&self, path: &str) -> String {
        format!(
            "http://{}/{}",
            self.control_host,
            path.trim_start_matches('/')
        )
    }

    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}
