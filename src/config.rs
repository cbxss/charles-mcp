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

    /// How long (ms) to cache the live session so a burst of inspect calls
    /// doesn't re-export Charles every time — this also keeps request indices
    /// stable within the window. Set to 0 to disable caching.
    #[arg(long, env = "CHARLES_CACHE_TTL_MS", default_value_t = 5_000)]
    pub cache_ttl_ms: u64,

    /// Timeout (ms) for the `charles convert` subprocess, so a license/GUI
    /// prompt can't hang a tool call forever.
    #[arg(long, env = "CHARLES_CONVERT_TIMEOUT_MS", default_value_t = 60_000)]
    pub convert_timeout_ms: u64,

    /// Timeout (ms) for fetching the *whole* live session (the export-json /
    /// native-download read), separate from `--timeout-ms`: a real 50+ MB
    /// capture takes far longer than a control call, and the per-request timeout
    /// would abort it and mis-report it as "endpoint not found".
    #[arg(long, env = "CHARLES_EXPORT_TIMEOUT_MS", default_value_t = 60_000)]
    pub export_timeout_ms: u64,

    /// Preferred format when fetching/exporting the live session.
    #[arg(long, env = "CHARLES_EXPORT_FORMAT", default_value = "chlsj")]
    pub default_export_format: String,

    /// Directory of `.proto` files for NAMED protobuf/gRPC field decoding
    /// (optional; schemaless decoding works without it).
    #[arg(long, env = "CHARLES_PROTO_DIR")]
    pub proto_dir: Option<PathBuf>,

    /// Path to the SQLite traffic store. When unset the store is ephemeral (a
    /// temp file deleted on exit); set this to persist captures across restarts.
    #[arg(long, env = "CHARLES_DB_PATH")]
    pub db_path: Option<PathBuf>,

    /// Max stored FILE captures kept in the store; the least-recently-used are
    /// evicted past this (the live capture is always retained). Bounds disk use.
    #[arg(long, env = "CHARLES_STORE_MAX_CAPTURES", default_value_t = 10)]
    pub store_max_captures: usize,

    /// Cap (bytes) on the decoded body text indexed per message for full-text
    /// search — keeps the FTS index bounded on large bodies.
    #[arg(long, env = "CHARLES_FTS_BODY_MAX_BYTES", default_value_t = 65_536)]
    pub fts_body_max_bytes: usize,
}

impl Config {
    /// `http://host:port` URL of the Charles proxy (brackets IPv6 hosts).
    pub fn proxy_url(&self) -> String {
        let host = &self.proxy_host;
        if host.contains(':') && !host.starts_with('[') {
            format!("http://[{host}]:{}", self.proxy_port)
        } else {
            format!("http://{host}:{}", self.proxy_port)
        }
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

    pub fn cache_ttl(&self) -> Duration {
        Duration::from_millis(self.cache_ttl_ms)
    }

    pub fn convert_timeout(&self) -> Duration {
        Duration::from_millis(self.convert_timeout_ms)
    }

    pub fn export_timeout(&self) -> Duration {
        Duration::from_millis(self.export_timeout_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(host: &str) -> Config {
        let mut c = Config::parse_from(["charles-mcp"]);
        c.proxy_host = host.to_string();
        c
    }

    #[test]
    fn proxy_url_brackets_ipv6() {
        assert_eq!(cfg("127.0.0.1").proxy_url(), "http://127.0.0.1:8888");
        assert_eq!(cfg("::1").proxy_url(), "http://[::1]:8888");
        assert_eq!(cfg("[::1]").proxy_url(), "http://[::1]:8888");
    }
}
