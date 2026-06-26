//! The live read/control path: turn the running Charles session into bytes we
//! can parse, and invoke the destructive session/quit actions.
//!
//! Layered and defensive, since the export/clear/quit endpoints are undocumented:
//!   1. parse the control page (`discovery`) and use the discovered endpoint;
//!   2. fall back to a small set of candidate paths;
//!   3. for parsing, fall back to downloading the native `.chls` and running
//!      `charles convert`.

use reqwest::StatusCode;

use super::WebClient;
use crate::error::CharlesError;
use crate::session::{Session, SessionSource, convert, sniff};
use crate::web::discovery::{self, DiscoveredEndpoints, EndpointSpec};

impl WebClient {
    /// Parse the control page once and cache the discovered endpoints.
    pub async fn discovered(&self) -> Result<DiscoveredEndpoints, CharlesError> {
        if let Some(d) = self.discovery.lock().await.clone() {
            return Ok(d);
        }
        let html = self.get_control_text("").await?;
        let d = discovery::discover_from_html(&html);
        *self.discovery.lock().await = Some(d.clone());
        Ok(d)
    }

    /// Drop the cached discovery (call after an endpoint unexpectedly 404s).
    pub async fn invalidate_discovery(&self) {
        *self.discovery.lock().await = None;
    }

    /// Low-level control request returning (status, body) or `None` on a
    /// transport error. Does not enforce success.
    async fn raw_request(
        &self,
        method: &str,
        path: &str,
        form: Option<&[(&str, &str)]>,
    ) -> Option<(StatusCode, Vec<u8>)> {
        let url = self.config().control_url(path);
        let mut req = if method.eq_ignore_ascii_case("POST") {
            self.http.post(&url)
        } else {
            self.http.get(&url)
        };
        if let Some(f) = form {
            req = req.form(f);
        }
        if let Some(user) = &self.config().web_user {
            req = req.basic_auth(user, self.config().web_pass.clone());
        }
        let resp = req.send().await.ok()?;
        let status = resp.status();
        let bytes = resp.bytes().await.ok()?.to_vec();
        Some((status, bytes))
    }

    /// Fetch data: success status and a non-empty body.
    async fn fetch_data(
        &self,
        method: &str,
        path: &str,
        form: Option<&[(&str, &str)]>,
    ) -> Option<Vec<u8>> {
        let (status, bytes) = self.raw_request(method, path, form).await?;
        (status.is_success() && !bytes.is_empty()).then_some(bytes)
    }

    /// Invoke an action (clear/quit): only the status matters.
    async fn invoke(&self, spec: &EndpointSpec) -> bool {
        match self.raw_request(&spec.method, &spec.path, None).await {
            Some((status, _)) => status.is_success(),
            // A quit that drops the connection looks like a transport error;
            // callers treat the candidate fallbacks accordingly.
            None => false,
        }
    }

    /// Fetch the live session and parse it. Prefers chlsj, then har, then the
    /// native `.chls` + `charles convert` fallback.
    pub async fn fetch_live_session(&self) -> Result<Session, CharlesError> {
        // Surface unreachable/auth problems with a clear message before we start
        // probing export endpoints (otherwise the user sees a misleading
        // "endpoint not found" for the last fallback).
        self.discovered().await?;
        for fmt in ["chlsj", "har"] {
            if let Ok(bytes) = self.fetch_session_in_format(fmt).await
                && let Ok(transactions) = sniff::parse_bytes(bytes)
            {
                return Ok(Session {
                    source: SessionSource::Live,
                    transactions,
                });
            }
        }
        // Native download + convert (requires the Charles binary).
        let chls = self.download_native().await?;
        let chlsj = convert::convert_bytes(self.config(), &chls, "chls").await?;
        let transactions = sniff::parse_bytes(chlsj)?;
        Ok(Session {
            source: SessionSource::Live,
            transactions,
        })
    }

    /// Fetch the current session as bytes in `format` (chlsj/har/xml/csv/chls).
    pub async fn fetch_session_in_format(&self, format: &str) -> Result<Vec<u8>, CharlesError> {
        if format.eq_ignore_ascii_case("chls") {
            return self.download_native().await;
        }

        // 1. Discovered export endpoint (propagates unreachable/auth errors).
        let d = self.discovered().await?;
        if let Some(exp) = &d.export {
            let format_ok = exp.formats.is_empty()
                || exp.formats.iter().any(|f| f.eq_ignore_ascii_case(format));
            if format_ok {
                if let Some(bytes) = self.request_export(exp, format).await {
                    return Ok(bytes);
                }
                self.invalidate_discovery().await;
            }
        }

        // 2. Candidate export paths (idempotent GETs).
        for path in candidate_export_paths(format) {
            if let Some(bytes) = self.fetch_data("GET", &path, None).await {
                return Ok(bytes);
            }
        }

        Err(CharlesError::EndpointNotFound("session export"))
    }

    /// Download the native `.chls` session bytes.
    pub async fn download_native(&self) -> Result<Vec<u8>, CharlesError> {
        let d = self.discovered().await?;
        if let Some(dl) = &d.download_chls {
            let method = dl.method.clone();
            if let Some(bytes) = self.fetch_data(&method, &dl.path, None).await {
                return Ok(bytes);
            }
            self.invalidate_discovery().await;
        }
        for path in [
            "session/download-session",
            "session/download",
            "session/export-session?format=chls",
        ] {
            if let Some(bytes) = self.fetch_data("GET", path, None).await {
                return Ok(bytes);
            }
        }
        Err(CharlesError::EndpointNotFound("native session download"))
    }

    /// Issue the discovered (or a candidate) export request for one format.
    async fn request_export(&self, exp: &EndpointSpec, format: &str) -> Option<Vec<u8>> {
        if exp.method.eq_ignore_ascii_case("POST") {
            let owned: Vec<(String, String)> = exp
                .format_field
                .iter()
                .map(|n| (n.clone(), format.to_string()))
                .collect();
            let form: Vec<(&str, &str)> = owned
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            self.fetch_data("POST", &exp.path, Some(&form)).await
        } else {
            self.fetch_data("GET", &export_get_path(exp, format), None)
                .await
        }
    }

    /// Clear the current session (destructive).
    pub async fn clear_session(&self) -> Result<(), CharlesError> {
        let d = self.discovered().await?;
        if let Some(ep) = &d.clear {
            if self.invoke(ep).await {
                return Ok(());
            }
            self.invalidate_discovery().await;
        }
        for path in ["session/clear-session", "session/clear"] {
            if let Some((status, _)) = self.raw_request("POST", path, None).await
                && status.is_success()
            {
                return Ok(());
            }
            if let Some((status, _)) = self.raw_request("GET", path, None).await
                && status.is_success()
            {
                return Ok(());
            }
        }
        Err(CharlesError::EndpointNotFound("session clear"))
    }

    /// Quit Charles (destructive).
    pub async fn quit_charles(&self) -> Result<(), CharlesError> {
        let d = self.discovered().await?;
        if let Some(ep) = &d.quit
            && self.invoke(ep).await
        {
            return Ok(());
        }
        for path in ["quit", "application/quit", "shutdown"] {
            if let Some((status, _)) = self.raw_request("GET", path, None).await
                && status.is_success()
            {
                return Ok(());
            }
        }
        Err(CharlesError::EndpointNotFound("quit"))
    }
}

/// Append the export format as a query param to a discovered GET endpoint.
fn export_get_path(exp: &EndpointSpec, format: &str) -> String {
    match &exp.format_field {
        Some(name) => {
            let q = url::form_urlencoded::Serializer::new(String::new())
                .append_pair(name, format)
                .finish();
            if exp.path.contains('?') {
                format!("{}&{}", exp.path, q)
            } else {
                format!("{}?{}", exp.path, q)
            }
        }
        None => exp.path.clone(),
    }
}

/// Best-effort candidate export paths derived from Charles's naming convention.
fn candidate_export_paths(format: &str) -> Vec<String> {
    vec![
        format!("session/export-session?format={format}"),
        format!("session/export?format={format}"),
        format!("session/export-session.{format}"),
        format!("session.{format}"),
    ]
}
