pub mod control;
pub mod discovery;
pub mod live;

use std::sync::Arc;
use std::time::Instant;

use reqwest::{Client, StatusCode};
use tokio::sync::Mutex;

use crate::config::Config;
use crate::error::CharlesError;
use crate::session::Session;
use crate::web::discovery::DiscoveredEndpoints;

#[derive(Clone)]
pub struct WebClient {
    cfg: Arc<Config>,
    http: Client,
    discovery: Arc<Mutex<Option<DiscoveredEndpoints>>>,
    live_cache: Arc<Mutex<Option<(Instant, Session)>>>,
}

#[derive(Debug, Clone)]
pub struct StatusReport {
    pub proxy: String,
    pub control_host: String,
    pub reachable: bool,
    pub authenticated: bool,
    pub charles_bin_present: bool,
    pub note: String,
}

impl WebClient {
    pub fn new(cfg: Arc<Config>) -> Result<Self, CharlesError> {
        let proxy = reqwest::Proxy::all(cfg.proxy_url())?;
        let http = Client::builder()
            .proxy(proxy)
            .timeout(cfg.timeout())
            .build()?;
        Ok(Self {
            cfg,
            http,
            discovery: Arc::new(Mutex::new(None)),
            live_cache: Arc::new(Mutex::new(None)),
        })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub async fn get_control_text(&self, path: &str) -> Result<String, CharlesError> {
        let resp = self.send_control(path).await?;
        Ok(resp.text().await?)
    }

    pub async fn get_control_bytes(
        &self,
        path: &str,
    ) -> Result<(StatusCode, Vec<u8>), CharlesError> {
        let url = self.cfg.control_url(path);
        let mut req = self.http.get(&url);
        if let Some(user) = &self.cfg.web_user {
            req = req.basic_auth(user, self.cfg.web_pass.clone());
        }
        let resp = req.send().await.map_err(|e| self.connect_err(e))?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        Ok((status, bytes.to_vec()))
    }

    async fn send_control(&self, path: &str) -> Result<reqwest::Response, CharlesError> {
        let url = self.cfg.control_url(path);
        let mut req = self.http.get(&url);
        if let Some(user) = &self.cfg.web_user {
            req = req.basic_auth(user, self.cfg.web_pass.clone());
        }
        let resp = req.send().await.map_err(|e| self.connect_err(e))?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(CharlesError::Unauthorized {
                realm: realm_from(&resp),
            });
        }
        if !status.is_success() {
            return Err(CharlesError::HttpStatus {
                status: status.as_u16(),
                path: path.to_string(),
            });
        }
        Ok(resp)
    }

    fn connect_err(&self, source: reqwest::Error) -> CharlesError {
        CharlesError::Unreachable {
            proxy: self.cfg.proxy_url(),
            source,
        }
    }

    pub async fn status(&self) -> StatusReport {
        let charles_bin_present = self.cfg.charles_bin.exists();
        let base = StatusReport {
            proxy: self.cfg.proxy_url(),
            control_host: self.cfg.control_host.clone(),
            reachable: false,
            authenticated: false,
            charles_bin_present,
            note: String::new(),
        };
        match self.get_control_text("").await {
            Ok(_) => {
                let authed = self.cfg.web_user.is_some();
                StatusReport {
                    reachable: true,
                    authenticated: authed,
                    note: if authed {
                        "Connected to the Charles Web Interface (authenticated).".into()
                    } else {
                        "Connected to the Charles Web Interface (anonymous access; no \
                         credentials sent)."
                            .into()
                    },
                    ..base
                }
            }
            Err(CharlesError::Unauthorized { realm }) => StatusReport {
                reachable: true,
                authenticated: false,
                note: format!(
                    "Proxy reachable but the Web Interface needs credentials{} \
                     (set --web-user/--web-pass).",
                    realm.map(|r| format!(" (realm {r:?})")).unwrap_or_default()
                ),
                ..base
            },
            Err(CharlesError::HttpStatus { status, .. }) => StatusReport {
                reachable: true,
                authenticated: true,
                note: format!("Web Interface answered with HTTP {status}."),
                ..base
            },
            Err(CharlesError::Unreachable { source, .. }) => {
                let note = if source.is_connect() {
                    "Cannot connect to the Charles proxy — is Charles running and is the \
                     proxy host/port correct?"
                        .into()
                } else if source.is_timeout() {
                    "Reached the proxy but the Web Interface did not respond (timeout). \
                     Is the Web Interface enabled in Proxy → Web Interface Settings?"
                        .into()
                } else {
                    format!("Cannot reach Charles: {source}")
                };
                StatusReport { note, ..base }
            }
            Err(e) => StatusReport {
                note: e.to_string(),
                ..base
            },
        }
    }
}

fn realm_from(resp: &reqwest::Response) -> Option<String> {
    let v = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)?
        .to_str()
        .ok()?;
    let idx = v.to_ascii_lowercase().find("realm=")?;
    let rest = v[idx + 6..].trim_start_matches('"');
    let end = rest.find('"').unwrap_or(rest.len());
    let realm = rest[..end].trim();
    (!realm.is_empty()).then(|| realm.to_string())
}
