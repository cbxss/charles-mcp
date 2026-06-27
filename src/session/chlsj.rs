//! Parser for Charles JSON session files (`.chlsj`) into the unified model.
//!
//! Schema confirmed against Charles output and the community Fiddler importer:
//! the top level is a JSON array; each element has `host`/`scheme`/`path`/
//! `query`/`method`/`protocolVersion` and `request`/`response` objects. The
//! HTTP status code lives at `response.status`; the top-level `status` is the
//! session *state* (e.g. COMPLETE/FAILED). Bodies are `body.text` (decoded) or
//! `body.encoded` (base64), with `contentEncoding` (gzip/brotli) as a sibling.
//! Parsing is deliberately tolerant: every field is optional with defaults.

use serde::Deserialize;
use serde_json::Value;

use super::{
    HttpMessage, RawBody, Transaction, WsDirection, charset_from_content_type, decode_base64,
    header_value, mime_essence, websocket,
};
use crate::error::CharlesError;

pub fn parse(bytes: &[u8]) -> Result<Vec<Transaction>, CharlesError> {
    let txns: Vec<ChlsTxn> =
        serde_json::from_slice(bytes).map_err(|e| CharlesError::Parse(format!("chlsj: {e}")))?;
    Ok(txns
        .into_iter()
        .enumerate()
        .map(|(i, t)| t.into_transaction(i))
        .collect())
}

#[derive(Deserialize, Default)]
struct ChlsTxn {
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    scheme: Option<String>,
    #[serde(default, rename = "actualPort")]
    actual_port: Option<u32>,
    #[serde(default)]
    port: Option<u32>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default, rename = "protocolVersion")]
    protocol_version: Option<String>,
    /// Session state (COMPLETE / FAILED / ...), not the HTTP status.
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "remoteAddress")]
    remote_address: Option<String>,
    #[serde(default, rename = "clientAddress")]
    client_address: Option<String>,
    /// TLS info lives under `ssl: { protocol }` in real Charles output.
    #[serde(default)]
    ssl: Option<ChlsSsl>,
    /// Human-readable failure message (e.g. "SSL handshake with client failed…").
    #[serde(default, rename = "errorMessage")]
    error_message: Option<String>,
    #[serde(default)]
    times: Option<Value>,
    #[serde(default)]
    durations: Option<Value>,
    #[serde(default)]
    request: Option<ChlsMessage>,
    #[serde(default)]
    response: Option<ChlsMessage>,
    /// True when Charles only saw an undecrypted HTTPS CONNECT tunnel (SSL
    /// Proxying not enabled for the host) — the body is ciphertext.
    #[serde(default)]
    tunnel: bool,
    /// True when this is a WebSocket connection; frames are the request/response
    /// bodies (raw RFC 6455, base64).
    #[serde(default, rename = "webSocket")]
    web_socket: bool,
}

#[derive(Deserialize, Default)]
struct ChlsSsl {
    #[serde(default)]
    protocol: Option<String>,
}

#[derive(Deserialize, Default)]
struct ChlsMessage {
    /// HTTP status code (present on the response side).
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    header: Option<ChlsHeader>,
    #[serde(default)]
    body: Option<ChlsBody>,
    #[serde(default)]
    sizes: Option<ChlsSizes>,
    #[serde(default, rename = "contentEncoding")]
    content_encoding: Option<String>,
    /// Some Charles versions put the MIME/charset on the message, not the body.
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(default)]
    charset: Option<String>,
}

#[derive(Deserialize, Default)]
struct ChlsHeader {
    #[serde(default)]
    headers: Vec<ChlsHeaderElem>,
}

#[derive(Deserialize)]
struct ChlsHeaderElem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
}

#[derive(Deserialize, Default)]
struct ChlsBody {
    #[serde(default)]
    encoded: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    charset: Option<String>,
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Deserialize, Default)]
struct ChlsSizes {
    #[serde(default)]
    body: Option<u64>,
}

/// Charles uses `brotli`; HTTP/our decoder expects `br`.
fn normalize_encoding(e: Option<String>) -> Option<String> {
    e.map(|s| match s.to_ascii_lowercase().as_str() {
        "brotli" => "br".to_string(),
        other => other.to_string(),
    })
    .filter(|s| !s.is_empty() && s != "identity" && s != "none")
}

impl ChlsMessage {
    /// Build an HttpMessage; returns (message, mime, body_size).
    fn into_http(self) -> (HttpMessage, Option<String>, Option<u64>) {
        let headers: Vec<(String, String)> = self
            .header
            .map(|h| h.headers.into_iter().map(|e| (e.name, e.value)).collect())
            .unwrap_or_default();

        let ct_header = header_value(&headers, "content-type").map(str::to_string);
        let header_ce = header_value(&headers, "content-encoding").map(str::to_string);
        // MIME/charset can live on the header, the message, or the body
        // depending on Charles version — try all three.
        let msg_mime = self.mime_type.clone();
        let msg_charset = self.charset.clone();

        let mut raw = RawBody {
            captured: false,
            ..Default::default()
        };
        let mut mime = mime_essence(ct_header.as_deref().or(msg_mime.as_deref()));
        let mut body_size = self.sizes.and_then(|s| s.body);

        if let Some(body) = self.body {
            let ct = ct_header
                .clone()
                .or_else(|| msg_mime.clone())
                .or_else(|| body.mime_type.clone());
            mime = mime_essence(ct.as_deref()).or(mime);
            raw.content_type = ct.clone();
            raw.declared_charset = body
                .charset
                .clone()
                .or_else(|| msg_charset.clone())
                .or_else(|| charset_from_content_type(ct.as_deref()));
            raw.content_encoding =
                normalize_encoding(self.content_encoding.clone()).or_else(|| header_ce.clone());
            if let Some(sz) = body.size {
                body_size = Some(sz);
            }

            match (body.encoded, body.text) {
                (Some(enc), _) if !enc.is_empty() => {
                    raw.bytes = decode_base64(&enc);
                    raw.was_base64_wrapped = true;
                    raw.captured = true;
                }
                (_, Some(text)) => {
                    // text is already decoded/decompressed.
                    raw.captured = !text.is_empty();
                    raw.bytes = text.into_bytes();
                    raw.content_encoding = None;
                }
                // Neither an encoded nor a text body was stored.
                _ => raw.captured = false,
            }
        } else {
            raw.content_type = ct_header.clone();
            raw.content_encoding = normalize_encoding(self.content_encoding).or(header_ce);
        }

        (HttpMessage { headers, raw }, mime, body_size)
    }
}

impl ChlsTxn {
    fn into_transaction(self, index: usize) -> Transaction {
        let scheme = self.scheme.unwrap_or_default();
        let host = self.host.unwrap_or_default();
        let port = self.actual_port.or(self.port);

        let mut path = self.path.unwrap_or_default();
        if let Some(q) = self.query.as_deref().filter(|q| !q.is_empty()) {
            path.push('?');
            path.push_str(q);
        }

        let mut url = format!("{scheme}://{host}");
        if let Some(p) = port {
            let default = (scheme == "http" && p == 80) || (scheme == "https" && p == 443);
            if !default {
                url.push_str(&format!(":{p}"));
            }
        }
        url.push_str(&path);

        let (request, _req_mime, _req_size) = self
            .request
            .map(ChlsMessage::into_http)
            .unwrap_or_else(|| (HttpMessage::default(), None, None));

        let mut status = None;
        let mut mime = None;
        let mut response_size = None;
        let response = self.response.map(|r| {
            status = r.status;
            let (msg, m, sz) = r.into_http();
            mime = m;
            response_size = sz;
            msg
        });

        // Prefer Charles's human-readable errorMessage; fall back to the state.
        let error = self.error_message.clone().or_else(|| {
            self.status
                .as_deref()
                .filter(|s| is_failed_state(s))
                .map(str::to_string)
        });

        // For a WebSocket connection the request/response bodies are the raw
        // RFC 6455 frame streams (sent = masked, received = unmasked).
        let websocket = self.web_socket.then(|| {
            let mut msgs = websocket::parse_messages(&request.raw.bytes, WsDirection::Sent);
            if let Some(resp) = &response {
                msgs.extend(websocket::parse_messages(
                    &resp.raw.bytes,
                    WsDirection::Received,
                ));
            }
            msgs
        });

        let tls_version = self.ssl.and_then(|s| s.protocol);
        let started = self.times.as_ref().and_then(parse_time_start);
        let duration_ms = self.durations.as_ref().and_then(|d| get_f64(d, "total"));

        Transaction {
            index,
            started,
            duration_ms,
            scheme,
            host,
            method: self.method.unwrap_or_default(),
            path,
            url,
            status,
            status_text: None,
            mime,
            response_size,
            protocol: self.protocol_version,
            client_addr: self.client_address,
            remote_addr: self.remote_address,
            tls_version,
            tunnel: self.tunnel,
            error,
            request,
            response,
            websocket,
        }
    }
}

fn is_failed_state(s: &str) -> bool {
    // Charles's real failure state is EXCEPTION; the rest are defensive in case
    // a version uses different wording. (COMPLETE/SUCCESS/RECEIVING_* are fine.)
    let s = s.to_ascii_uppercase();
    s.contains("EXCEPTION")
        || s.contains("FAIL")
        || s.contains("ABORT")
        || s.contains("ERROR")
        || s.contains("TIMEOUT")
}

fn get_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(|x| match x {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

/// Parse the `start` field of a chlsj `times` object (ISO-8601 or epoch).
fn parse_time_start(times: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    let start = times.get("start")?;
    match start {
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc)),
        Value::Number(n) => {
            let n = n.as_f64()?;
            let millis = if n > 1e12 {
                n as i64
            } else {
                (n * 1000.0) as i64
            };
            chrono::DateTime::from_timestamp_millis(millis)
        }
        _ => None,
    }
}
