use std::path::PathBuf;
use std::time::Duration;
use std::{env, path::Path};

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "charles-mcp",
    version,
    about = "MCP server for Charles Proxy 5"
)]
pub struct Config {
    #[arg(long, env = "CHARLES_PROXY_HOST", default_value = "127.0.0.1")]
    pub proxy_host: String,

    #[arg(long, env = "CHARLES_PROXY_PORT", default_value_t = 8888)]
    pub proxy_port: u16,

    #[arg(long, env = "CHARLES_CONTROL_HOST", default_value = "control.charles")]
    pub control_host: String,

    #[arg(long, env = "CHARLES_WEB_USER")]
    pub web_user: Option<String>,

    #[arg(long, env = "CHARLES_WEB_PASS")]
    pub web_pass: Option<String>,

    #[arg(
        long,
        env = "CHARLES_BIN",
        default_value = "/Applications/Charles.app/Contents/MacOS/Charles"
    )]
    pub charles_bin: PathBuf,

    #[arg(long, env = "CHARLES_TIMEOUT_MS", default_value_t = 15_000)]
    pub timeout_ms: u64,

    #[arg(long, env = "CHARLES_BODY_MAX_BYTES", default_value_t = 8_192)]
    pub body_max_bytes: usize,

    #[arg(long, env = "CHARLES_CACHE_TTL_MS", default_value_t = 5_000)]
    pub cache_ttl_ms: u64,

    #[arg(long, env = "CHARLES_CONVERT_TIMEOUT_MS", default_value_t = 60_000)]
    pub convert_timeout_ms: u64,

    #[arg(long, env = "CHARLES_EXPORT_TIMEOUT_MS", default_value_t = 60_000)]
    pub export_timeout_ms: u64,

    #[arg(long, env = "CHARLES_EXPORT_FORMAT", default_value = "chlsj")]
    pub default_export_format: String,

    #[arg(long, env = "CHARLES_DB_PATH")]
    pub db_path: Option<PathBuf>,

    #[arg(long, env = "CHARLES_STORE_MAX_CAPTURES", default_value_t = 10)]
    pub store_max_captures: usize,

    #[arg(long, env = "CHARLES_FTS_BODY_MAX_BYTES", default_value_t = 65_536)]
    pub fts_body_max_bytes: usize,

    #[arg(long, env = "CHARLES_INCLUDE_CONTROL_TRAFFIC", default_value_t = false)]
    pub include_control_traffic: bool,

    #[arg(long, env = "CHARLES_CONFIG_PATH")]
    pub charles_config_path: Option<PathBuf>,
}

impl Config {
    pub fn proxy_url(&self) -> String {
        let host = &self.proxy_host;
        if host.contains(':') && !host.starts_with('[') {
            format!("http://[{host}]:{}", self.proxy_port)
        } else {
            format!("http://{host}:{}", self.proxy_port)
        }
    }

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

    pub fn resolved_charles_config_path(&self) -> Option<PathBuf> {
        if let Some(path) = &self.charles_config_path {
            return Some(path.clone());
        }
        platform_default_config_path()
    }
}

fn platform_default_config_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return home_path("Library/Preferences/com.xk72.charles.config");
    }
    if cfg!(target_os = "windows") {
        return env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("Charles").join("charles.config"));
    }
    home_path(".charles.config")
}

fn home_path(suffix: &str) -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|p| p.join(Path::new(suffix)))
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
