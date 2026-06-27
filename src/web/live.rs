use reqwest::StatusCode;

use super::WebClient;
use crate::error::CharlesError;
use crate::session::{
    Session, SessionSource, Transaction, convert, looks_like_schema_mismatch, sniff,
};
use crate::web::discovery::{self, DiscoveredEndpoints, EndpointSpec};

impl WebClient {
    pub async fn discovered(&self) -> Result<DiscoveredEndpoints, CharlesError> {
        if let Some(d) = self.discovery.lock().await.clone() {
            return Ok(d);
        }
        let html = self.get_control_text("").await?;
        let d = discovery::discover_from_html(&html);
        *self.discovery.lock().await = Some(d.clone());
        Ok(d)
    }

    pub async fn invalidate_discovery(&self) {
        *self.discovery.lock().await = None;
    }

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

    async fn fetch_data(
        &self,
        method: &str,
        path: &str,
        form: Option<&[(&str, &str)]>,
    ) -> Option<Vec<u8>> {
        let (status, bytes) = self.raw_request(method, path, form).await?;
        (status.is_success() && !bytes.is_empty()).then_some(bytes)
    }

    async fn invoke(&self, spec: &EndpointSpec) -> bool {
        match self.raw_request(&spec.method, &spec.path, None).await {
            Some((status, _)) => status.is_success(),
            None => false,
        }
    }

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

    pub async fn invalidate_live_cache(&self) {
        *self.live_cache.lock().await = None;
    }

    async fn fetch_live_session_uncached(&self) -> Result<Session, CharlesError> {
        self.discovered().await?;

        if let Some(transactions) = self.try_export_json().await {
            if session_has_unframed_websocket(&transactions)
                && let Ok(session) = self.fetch_via_native_convert().await
            {
                return Ok(session);
            }
            return Ok(Session {
                source: SessionSource::Live,
                transactions,
            });
        }

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

        self.fetch_via_native_convert().await
    }

    async fn try_export_json(&self) -> Option<Vec<Transaction>> {
        let url = self.config().control_url("session/export-json");
        let mut req = self.http.get(&url).timeout(self.config().export_timeout());
        if let Some(user) = &self.config().web_user {
            req = req.basic_auth(user, self.config().web_pass.clone());
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let bytes = resp.bytes().await.ok()?.to_vec();
        if bytes.is_empty() {
            return None;
        }
        let transactions = sniff::parse_bytes(bytes).ok()?;
        if looks_like_schema_mismatch(&transactions) {
            return None;
        }
        Some(transactions)
    }

    async fn fetch_via_native_convert(&self) -> Result<Session, CharlesError> {
        let native = self.download_native().await?;
        let chlsj =
            convert::convert_bytes(self.config(), &native, native_ext(&native), "chlsj").await?;
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

    pub async fn fetch_session_in_format(&self, format: &str) -> Result<Vec<u8>, CharlesError> {
        if format.eq_ignore_ascii_case("chls") {
            return self.download_native().await;
        }

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

        for path in candidate_export_paths(format) {
            if let Some(bytes) = self.fetch_data("GET", &path, None).await {
                return Ok(bytes);
            }
        }

        if let Ok(native) = self.download_native().await
            && let Ok(bytes) =
                convert::convert_bytes(self.config(), &native, native_ext(&native), format).await
        {
            return Ok(bytes);
        }

        Err(CharlesError::EndpointNotFound("session export"))
    }

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

    pub async fn quit_charles(&self) -> Result<(), CharlesError> {
        let d = self.discovered().await?;
        if let Some(ep) = &d.quit {
            let _ = self.raw_request(&ep.method, &ep.path, None).await;
        } else {
            for path in ["quit", "application/quit", "shutdown"] {
                let _ = self.raw_request("GET", path, None).await;
            }
        }
        match self.get_control_text("").await {
            Err(CharlesError::Unreachable { .. }) => {
                self.invalidate_live_cache().await;
                self.invalidate_discovery().await;
                Ok(())
            }
            _ => Err(CharlesError::EndpointNotFound("quit")),
        }
    }
}

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

fn session_has_unframed_websocket(txns: &[Transaction]) -> bool {
    txns.iter()
        .any(|t| is_websocket_upgrade(t) && t.websocket.as_ref().is_none_or(|f| f.is_empty()))
}

fn is_websocket_upgrade(t: &Transaction) -> bool {
    if t.status == Some(101) {
        return true;
    }
    let upgrades = |m: &crate::session::HttpMessage| {
        m.header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    };
    upgrades(&t.request) || t.response.as_ref().is_some_and(upgrades)
}

fn native_ext(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"PK\x03\x04") {
        "chlz"
    } else {
        "chls"
    }
}

fn candidate_export_paths(format: &str) -> Vec<String> {
    vec![
        format!("session/export-session?format={format}"),
        format!("session/export?format={format}"),
        format!("session/export-session.{format}"),
        format!("session.{format}"),
    ]
}
