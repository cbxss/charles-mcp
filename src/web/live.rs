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
use crate::session::{Session, SessionSource, convert, looks_like_schema_mismatch, sniff};
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

    /// Fetch the live session and parse it, served from a short-lived cache so a
    /// burst of inspect calls doesn't re-export Charles every time (this also
    /// keeps request indices stable within the TTL window).
    pub async fn fetch_live_session(&self) -> Result<Session, CharlesError> {
        let ttl = self.config().cache_ttl();
        if !ttl.is_zero()
            && let Some((at, sess)) = self.live_cache.lock().await.as_ref()
            && at.elapsed() < ttl
        {
            return Ok(sess.clone());
        }
        let session = self.fetch_live_session_uncached().await?;
        if !ttl.is_zero() {
            *self.live_cache.lock().await = Some((std::time::Instant::now(), session.clone()));
        }
        Ok(session)
    }

    /// Drop the cached live session (after a clear, or to force a refresh).
    pub async fn invalidate_live_cache(&self) {
        *self.live_cache.lock().await = None;
    }

    /// Prefers chlsj, then har, then native `.chls` + `charles convert`.
    async fn fetch_live_session_uncached(&self) -> Result<Session, CharlesError> {
        // Surface unreachable/auth problems with a clear message before we start
        // probing export endpoints.
        self.discovered().await?;
        for fmt in ["chlsj", "har"] {
            if let Ok(bytes) = self.fetch_session_in_format(fmt).await
                && let Ok(transactions) = sniff::parse_bytes(bytes)
                && !looks_like_schema_mismatch(&transactions)
            {
                return Ok(Session {
                    source: SessionSource::Live,
                    transactions,
                });
            }
        }
        // Native download + convert (requires the Charles binary).
        let chls = self.download_native().await?;
        let chlsj = convert::convert_bytes(self.config(), &chls, "chls", "chlsj").await?;
        let transactions = sniff::parse_bytes(chlsj)?;
        if looks_like_schema_mismatch(&transactions) {
            return Err(CharlesError::Parse(
                "exported the live session but every host/method is empty — the .chlsj schema \
                 does not match this Charles version"
                    .into(),
            ));
        }
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

        // 3. Last resort: download the native .chls and convert it locally to the
        //    requested format (needs the Charles binary). This is what makes
        //    export to har/chlsj robust even if the web-export endpoint is absent.
        if let Ok(chls) = self.download_native().await
            && let Ok(bytes) = convert::convert_bytes(self.config(), &chls, "chls", format).await
        {
            return Ok(bytes);
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

    /// Clear the current session (destructive). Invalidates the live cache so the
    /// next inspect call reflects the now-empty session.
    pub async fn clear_session(&self) -> Result<(), CharlesError> {
        let d = self.discovered().await?;
        let cleared = match &d.clear {
            Some(ep) if self.invoke(ep).await => true,
            _ => {
                self.invalidate_discovery().await;
                self.try_clear_candidates().await
            }
        };
        if cleared {
            self.invalidate_live_cache().await;
            Ok(())
        } else {
            Err(CharlesError::EndpointNotFound("session clear"))
        }
    }

    async fn try_clear_candidates(&self) -> bool {
        for path in ["session/clear-session", "session/clear"] {
            for method in ["POST", "GET"] {
                if let Some((status, _)) = self.raw_request(method, path, None).await
                    && status.is_success()
                {
                    return true;
                }
            }
        }
        false
    }

    /// Quit Charles (destructive).
    pub async fn quit_charles(&self) -> Result<(), CharlesError> {
        let d = self.discovered().await?;
        // Fire the quit request best-effort. A *successful* quit tears down the
        // proxy mid-request, so its return is unreliable — we verify by
        // connectivity instead of trusting the response (the old bug reported a
        // working quit as a failure and invited a retry).
        if let Some(ep) = &d.quit {
            let _ = self.raw_request(&ep.method, &ep.path, None).await;
        } else {
            for path in ["quit", "application/quit", "shutdown"] {
                let _ = self.raw_request("GET", path, None).await;
            }
        }
        match self.get_control_text("").await {
            // Control host no longer reachable → Charles quit. Success.
            Err(CharlesError::Unreachable { .. }) => {
                self.invalidate_live_cache().await;
                self.invalidate_discovery().await;
                Ok(())
            }
            // Still reachable → the quit endpoint was wrong/unsupported.
            _ => Err(CharlesError::EndpointNotFound("quit")),
        }
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
