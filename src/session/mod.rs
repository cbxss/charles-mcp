//! Unified, normalized session model that both the HAR and `.chlsj` parsers
//! target, plus the format sniffer, body decoder, and `charles convert` shim.

pub mod body;
pub mod chlsj;
pub mod convert;
pub mod har;
pub mod sniff;

use std::path::PathBuf;

use base64::Engine as _;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub enum SessionSource {
    Live,
    File(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Session {
    pub source: SessionSource,
    pub transactions: Vec<Transaction>,
}

impl Session {
    pub fn get(&self, index: usize) -> Option<&Transaction> {
        self.transactions.get(index)
    }

    pub fn summaries(&self) -> Vec<TxnSummary> {
        self.transactions.iter().map(Transaction::summary).collect()
    }
}

/// One normalized request/response exchange.
#[derive(Debug, Clone, Default)]
pub struct Transaction {
    pub index: usize,
    pub started: Option<DateTime<Utc>>,
    pub duration_ms: Option<f64>,
    pub scheme: String,
    pub host: String,
    pub method: String,
    /// Path including the query string (e.g. `/api?x=1`).
    pub path: String,
    pub url: String,
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub mime: Option<String>,
    pub response_size: Option<u64>,
    pub protocol: Option<String>,
    pub client_addr: Option<String>,
    pub remote_addr: Option<String>,
    pub tls_version: Option<String>,
    /// True for an HTTPS CONNECT tunnel Charles did NOT decrypt (SSL Proxying
    /// not enabled for this host): any captured body is ciphertext, not real
    /// content. Surfaced so the agent isn't misled into thinking the body was
    /// simply "not captured".
    pub tunnel: bool,
    /// Set when the transaction failed/was aborted (chlsj session state).
    pub error: Option<String>,
    pub request: HttpMessage,
    pub response: Option<HttpMessage>,
}

impl Transaction {
    pub fn summary(&self) -> TxnSummary {
        TxnSummary {
            index: self.index,
            method: self.method.clone(),
            status: self.status,
            host: self.host.clone(),
            path: self.path.clone(),
            mime: self.mime.clone(),
            response_size: self.response_size,
            duration_ms: self.duration_ms,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HttpMessage {
    /// Ordered headers; duplicates preserved.
    pub headers: Vec<(String, String)>,
    pub raw: RawBody,
}

impl HttpMessage {
    /// Case-insensitive header lookup (first match).
    pub fn header(&self, name: &str) -> Option<&str> {
        header_value(&self.headers, name)
    }
}

/// A body as captured, decoded lazily by [`body::decode`].
#[derive(Debug, Clone, Default)]
pub struct RawBody {
    /// Bytes after any base64 unwrapping (still possibly compressed).
    pub bytes: Vec<u8>,
    pub content_encoding: Option<String>,
    pub content_type: Option<String>,
    pub declared_charset: Option<String>,
    pub was_base64_wrapped: bool,
    /// False when Charles recorded that the body was not stored.
    pub captured: bool,
}

/// Presentation form of a body, produced on demand for `get_request`.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Empty,
    NotCaptured,
    Text {
        text: String,
        charset: String,
        truncated: bool,
        original_len: u64,
    },
    Binary {
        bytes_len: u64,
        sample_hex: String,
        truncated: bool,
    },
}

/// Compact, bodyless row used by list/search.
#[derive(Debug, Clone)]
pub struct TxnSummary {
    pub index: usize,
    pub method: String,
    pub status: Option<u16>,
    pub host: String,
    pub path: String,
    pub mime: Option<String>,
    pub response_size: Option<u64>,
    pub duration_ms: Option<f64>,
}

/// Extract `charset` from a `Content-Type` value, if present (case-insensitive).
pub fn charset_from_content_type(ct: Option<&str>) -> Option<String> {
    ct?.split(';').map(str::trim).find_map(|p| {
        p.get(..8)
            .filter(|pre| pre.eq_ignore_ascii_case("charset="))
            .map(|_| p[8..].trim_matches('"').to_string())
    })
}

/// Case-insensitive header lookup over an ordered header list (first match).
pub(crate) fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Heuristic guard against a silently-mismatched parse: if we got transactions
/// but every one has an empty host AND method, the input schema almost
/// certainly didn't match (serde filled all fields with defaults). Lets callers
/// turn "parsed garbage, no error" into a clear failure.
pub fn looks_like_schema_mismatch(txns: &[Transaction]) -> bool {
    !txns.is_empty()
        && txns
            .iter()
            .all(|t| t.host.is_empty() && t.method.is_empty())
}

/// Decode standard base64, yielding empty bytes on error (lenient by design).
pub(crate) fn decode_base64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .unwrap_or_default()
}

/// The essence (type/subtype) of a `Content-Type`, lower-cased, without params.
pub fn mime_essence(ct: Option<&str>) -> Option<String> {
    ct.map(|c| c.split(';').next().unwrap_or(c).trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}
