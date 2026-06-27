//! Parser for HAR 1.2 (`.har`) files into the unified [`Transaction`] model.

use serde::Deserialize;

use super::{
    HttpMessage, RawBody, Transaction, charset_from_content_type, decode_base64, header_value,
    mime_essence,
};
use crate::error::CharlesError;

pub fn parse(bytes: &[u8]) -> Result<Vec<Transaction>, CharlesError> {
    let har: Har =
        serde_json::from_slice(bytes).map_err(|e| CharlesError::Parse(format!("HAR: {e}")))?;
    Ok(har
        .log
        .entries
        .into_iter()
        .enumerate()
        .map(|(i, entry)| entry.into_transaction(i))
        .collect())
}

#[derive(Deserialize)]
struct Har {
    log: HarLog,
}

#[derive(Deserialize)]
struct HarLog {
    #[serde(default)]
    entries: Vec<HarEntry>,
}

#[derive(Deserialize)]
struct HarEntry {
    #[serde(default, rename = "startedDateTime")]
    started: Option<String>,
    #[serde(default)]
    time: Option<f64>,
    #[serde(default)]
    request: HarRequest,
    #[serde(default)]
    response: Option<HarResponse>,
    #[serde(default, rename = "serverIPAddress")]
    server_ip: Option<String>,
}

#[derive(Deserialize, Default)]
struct HarRequest {
    #[serde(default)]
    method: String,
    #[serde(default)]
    url: String,
    #[serde(default, rename = "httpVersion")]
    http_version: Option<String>,
    #[serde(default)]
    headers: Vec<HarHeader>,
    #[serde(default, rename = "postData")]
    post_data: Option<HarPostData>,
}

#[derive(Deserialize)]
struct HarResponse {
    #[serde(default)]
    status: u16,
    #[serde(default, rename = "statusText")]
    status_text: Option<String>,
    #[serde(default, rename = "httpVersion")]
    http_version: Option<String>,
    #[serde(default)]
    headers: Vec<HarHeader>,
    #[serde(default)]
    content: Option<HarContent>,
    #[serde(default, rename = "bodySize")]
    body_size: Option<i64>,
}

#[derive(Deserialize)]
struct HarHeader {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
}

#[derive(Deserialize)]
struct HarPostData {
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct HarContent {
    #[serde(default)]
    size: Option<i64>,
    #[serde(default, rename = "mimeType")]
    mime_type: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    encoding: Option<String>,
}

fn headers_vec(hs: Vec<HarHeader>) -> Vec<(String, String)> {
    hs.into_iter().map(|h| (h.name, h.value)).collect()
}

fn non_negative(v: Option<i64>) -> Option<u64> {
    v.filter(|n| *n >= 0).map(|n| n as u64)
}

/// Build the request-side HttpMessage from HAR request headers + body.
fn build_request(headers: Vec<HarHeader>, post_data: Option<HarPostData>) -> HttpMessage {
    let headers = headers_vec(headers);
    let mut message = HttpMessage {
        headers: headers.clone(),
        raw: RawBody::default(),
    };
    if let Some(pd) = post_data {
        let mime = pd
            .mime_type
            .or_else(|| header_value(&headers, "content-type").map(str::to_string));
        let text = pd.text.unwrap_or_default();
        message.raw = RawBody {
            declared_charset: charset_from_content_type(mime.as_deref()),
            content_encoding: header_value(&headers, "content-encoding").map(str::to_string),
            grpc_encoding: header_value(&headers, "grpc-encoding").map(str::to_string),
            captured: !text.is_empty(),
            bytes: text.into_bytes(),
            content_type: mime,
            was_base64_wrapped: false,
        };
    }
    message
}

/// The response-derived fields of a transaction.
#[derive(Default)]
struct ResponseParts {
    message: Option<HttpMessage>,
    status: Option<u16>,
    status_text: Option<String>,
    mime: Option<String>,
    response_size: Option<u64>,
    protocol: Option<String>,
}

fn build_response(resp: HarResponse) -> ResponseParts {
    let headers = headers_vec(resp.headers);
    let ct_header = header_value(&headers, "content-type").map(str::to_string);
    let mut raw = RawBody {
        content_encoding: header_value(&headers, "content-encoding").map(str::to_string),
        captured: false,
        ..Default::default()
    };

    let (mime, response_size);
    if let Some(content) = resp.content {
        let ct = content.mime_type.clone().or(ct_header.clone());
        response_size = non_negative(content.size).or(non_negative(resp.body_size));
        match (content.text, content.encoding.as_deref()) {
            (Some(t), Some("base64")) => {
                raw.bytes = decode_base64(&t);
                raw.was_base64_wrapped = true;
                raw.captured = true;
            }
            (Some(t), _) => {
                raw.captured = !t.is_empty();
                raw.bytes = t.into_bytes();
            }
            // Body not stored by the recorder.
            (None, _) => raw.captured = false,
        }
        raw.declared_charset = charset_from_content_type(ct.as_deref());
        mime = mime_essence(ct.as_deref());
        raw.content_type = ct;
    } else {
        mime = mime_essence(ct_header.as_deref());
        response_size = non_negative(resp.body_size);
        raw.content_type = ct_header;
    }

    ResponseParts {
        message: Some(HttpMessage { headers, raw }),
        status: (resp.status != 0).then_some(resp.status),
        status_text: resp.status_text.filter(|s| !s.is_empty()),
        mime,
        response_size,
        protocol: resp.http_version.filter(|s| !s.is_empty()),
    }
}

impl HarEntry {
    fn into_transaction(self, index: usize) -> Transaction {
        let HarEntry {
            started,
            time,
            request,
            response,
            server_ip,
        } = self;
        let HarRequest {
            method,
            url,
            http_version,
            headers,
            post_data,
        } = request;

        let (scheme, host, path) = split_url(&url);
        let request = build_request(headers, post_data);

        // Drop a fabricated response when the recorder captured nothing (status 0,
        // no body) so the detail view matches the chlsj parser's `None`.
        let parts = match response {
            Some(r)
                if r.status == 0 && r.content.as_ref().and_then(|c| c.text.as_ref()).is_none() =>
            {
                ResponseParts::default()
            }
            Some(r) => build_response(r),
            None => ResponseParts::default(),
        };

        Transaction {
            index,
            started: started
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc)),
            duration_ms: time,
            scheme,
            host,
            method,
            path,
            url,
            status: parts.status,
            status_text: parts.status_text,
            mime: parts.mime,
            response_size: parts.response_size,
            protocol: parts.protocol.or(http_version),
            client_addr: None,
            remote_addr: server_ip,
            tls_version: None,
            // HAR entries are already decrypted; no undecrypted-tunnel concept.
            tunnel: false,
            error: None,
            request,
            response: parts.message,
            websocket: None,
        }
    }
}

/// Split an absolute URL into (scheme, host, path-with-query).
pub(crate) fn split_url(raw: &str) -> (String, String, String) {
    match url::Url::parse(raw) {
        Ok(u) => {
            let mut path = u.path().to_string();
            if let Some(q) = u.query() {
                path.push('?');
                path.push_str(q);
            }
            (
                u.scheme().to_string(),
                u.host_str().unwrap_or("").to_string(),
                path,
            )
        }
        Err(_) => (String::new(), String::new(), raw.to_string()),
    }
}
