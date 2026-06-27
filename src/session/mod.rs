pub mod body;
pub mod chlsj;
pub mod classify;
pub mod convert;
pub mod grpc;
pub mod har;
pub mod protobuf;
pub mod sniff;
pub mod websocket;

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

#[derive(Debug, Clone, Default)]
pub struct Transaction {
    pub index: usize,
    pub started: Option<DateTime<Utc>>,
    pub duration_ms: Option<f64>,
    pub scheme: String,
    pub host: String,
    pub method: String,
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
    pub tunnel: bool,
    pub error: Option<String>,
    pub request: HttpMessage,
    pub response: Option<HttpMessage>,
    pub websocket: Option<Vec<WsMessage>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsDirection {
    Sent,
    Received,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WsOpcode {
    Text,
    Binary,
    Ping,
    Pong,
    Close,
    Other(u8),
}

#[derive(Debug, Clone)]
pub struct WsMessage {
    pub direction: WsDirection,
    pub opcode: WsOpcode,
    pub payload: RawBody,
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
    pub headers: Vec<(String, String)>,
    pub raw: RawBody,
}

impl HttpMessage {
    pub fn header(&self, name: &str) -> Option<&str> {
        header_value(&self.headers, name)
    }
}

#[derive(Debug, Clone, Default)]
pub struct RawBody {
    pub bytes: Vec<u8>,
    pub content_encoding: Option<String>,
    pub content_type: Option<String>,
    pub declared_charset: Option<String>,
    pub was_base64_wrapped: bool,
    pub captured: bool,
    pub grpc_encoding: Option<String>,
}

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
    Protobuf {
        tree: String,
        message_count: usize,
        named: bool,
        truncated: bool,
        original_len: u64,
    },
}

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

pub fn charset_from_content_type(ct: Option<&str>) -> Option<String> {
    ct?.split(';').map(str::trim).find_map(|p| {
        p.get(..8)
            .filter(|pre| pre.eq_ignore_ascii_case("charset="))
            .map(|_| p[8..].trim_matches('"').to_string())
    })
}

pub(crate) fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

pub fn looks_like_schema_mismatch(txns: &[Transaction]) -> bool {
    !txns.is_empty()
        && txns
            .iter()
            .all(|t| t.host.is_empty() && t.method.is_empty())
}

pub(crate) fn decode_base64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .unwrap_or_default()
}

pub fn mime_essence(ct: Option<&str>) -> Option<String> {
    ct.map(|c| c.split(';').next().unwrap_or(c).trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}
