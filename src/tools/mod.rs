//! Request parameter types for the MCP tools (wire schema lives here).

pub mod inspect;

use std::collections::HashMap;

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::web::control::CharlesTool;

/// Charles tool selector exposed to MCP callers (kebab-case on the wire).
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ToolName {
    Breakpoints,
    NoCaching,
    BlockCookies,
    MapRemote,
    MapLocal,
    Rewrite,
    BlackList,
    WhiteList,
    DnsSpoofing,
    AutoSave,
    ClientProcess,
}

impl ToolName {
    pub fn to_web(self) -> CharlesTool {
        match self {
            ToolName::Breakpoints => CharlesTool::Breakpoints,
            ToolName::NoCaching => CharlesTool::NoCaching,
            ToolName::BlockCookies => CharlesTool::BlockCookies,
            ToolName::MapRemote => CharlesTool::MapRemote,
            ToolName::MapLocal => CharlesTool::MapLocal,
            ToolName::Rewrite => CharlesTool::Rewrite,
            ToolName::BlackList => CharlesTool::BlackList,
            ToolName::WhiteList => CharlesTool::WhiteList,
            ToolName::DnsSpoofing => CharlesTool::DnsSpoofing,
            ToolName::AutoSave => CharlesTool::AutoSave,
            ToolName::ClientProcess => CharlesTool::ClientProcess,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ThrottlingReq {
    /// Turn throttling on (true) or off (false).
    pub enabled: bool,
    /// Optional Charles preset, e.g. "3G", "4G", or "56 kbps Modem".
    /// Omit to use the last/default preset.
    #[serde(default)]
    pub preset: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetToolReq {
    /// Which Charles tool to toggle.
    pub tool: ToolName,
    /// Enable (true) or disable (false).
    pub enabled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ToolNameReq {
    /// Which Charles tool to query.
    pub tool: ToolName,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfirmReq {
    /// Must be set to true to perform this destructive action.
    pub confirm: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResetReq {
    /// Must be set to true to drop all stored captures from the traffic store.
    pub confirm: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileReq {
    /// Absolute path to a .chls, .har, or .chlsj session file.
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRequestsReq {
    /// Only requests whose host contains this substring (case-insensitive).
    #[serde(default)]
    pub host: Option<String>,
    /// Only requests with this HTTP method (e.g. GET, POST).
    #[serde(default)]
    pub method: Option<String>,
    /// Only requests with this exact response status code.
    #[serde(default)]
    pub status: Option<u16>,
    /// Only requests whose path (incl. query) matches this regular expression.
    #[serde(default)]
    pub path_regex: Option<String>,
    /// Only responses whose MIME type contains this substring.
    #[serde(default)]
    pub mime: Option<String>,
    /// Only requests of this resource class: api_candidate, document, script,
    /// static_asset, font, media, connect_tunnel, control, or unknown. Use
    /// "api_candidate" to cut straight to the interesting API traffic.
    #[serde(default)]
    pub resource_class: Option<String>,
    /// Only requests whose priority score is at least this (api_candidates score
    /// highest; static assets lowest). Filters out low-signal noise.
    #[serde(default)]
    pub min_priority: Option<i64>,
    /// Maximum number of rows to return (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Inspect this session file instead of the live Charles session.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRequestReq {
    /// 0-based index of the request (from list_requests).
    pub index: usize,
    /// Cap on decoded body bytes shown (defaults to the server's body limit).
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
    /// Fully-qualified protobuf message type (e.g. "pkg.MyMessage") to decode the
    /// body with named fields, using the server's --proto-dir. Omit for the
    /// schemaless field-number tree.
    #[serde(default)]
    pub proto_type: Option<String>,
    /// Inspect this session file instead of the live Charles session.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WsMessagesReq {
    /// 0-based index of the WebSocket transaction (from list_requests).
    pub index: usize,
    /// Max frames to return (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Filter by direction: "sent" or "received". Omit for both.
    #[serde(default)]
    pub direction: Option<String>,
    /// Cap on decoded bytes per frame.
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
    /// Inspect this session file instead of the live Charles session.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchField {
    Url,
    Headers,
    Body,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchReq {
    /// Text (case-insensitive substring) or regular expression to search for.
    pub query: String,
    /// Treat `query` as a regular expression instead of a substring.
    #[serde(default)]
    pub regex: bool,
    /// Which parts to search: any of "url", "headers", "body" (default all).
    #[serde(default)]
    pub fields: Option<Vec<SearchField>>,
    /// Maximum number of hits to return (default 50).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Inspect this session file instead of the live Charles session.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatsReq {
    /// Inspect this session file instead of the live Charles session.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplayReq {
    /// 0-based index of the captured request to replay (from list_requests).
    pub index: usize,
    /// Must be true to actually send the request — replay makes a REAL network
    /// call to the origin server.
    pub confirm: bool,
    /// Must ALSO be true to replay a mutating method (POST/PUT/PATCH/DELETE),
    /// which may change server state. GET/HEAD do not require this.
    #[serde(default)]
    pub allow_mutating: bool,
    /// Override/add query parameters; a null value removes that parameter.
    #[serde(default)]
    pub query_overrides: Option<HashMap<String, Option<String>>>,
    /// Override/add request headers; a null value removes that header. The target
    /// host cannot be changed (it is fixed to the captured entry).
    #[serde(default)]
    pub header_overrides: Option<HashMap<String, Option<String>>>,
    /// Merge these keys into a JSON request body (a null value removes the key).
    /// Requires the original body to be JSON (or absent).
    #[serde(default)]
    pub json_overrides: Option<serde_json::Value>,
    /// Replace the entire request body with this exact text.
    #[serde(default)]
    pub body_text: Option<String>,
    /// Send the replay THROUGH the Charles proxy so it is re-captured in the live
    /// session (default false → sent directly to the origin).
    #[serde(default)]
    pub use_proxy: bool,
    /// Follow redirects (default false, so you see the raw 3xx).
    #[serde(default)]
    pub follow_redirects: bool,
    /// Cap on decoded response body bytes shown.
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
    /// Replay from this session file instead of the live Charles session.
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportReq {
    /// Output format: chlsj, har, chls, xml, or csv.
    pub format: SessionFormat,
    /// Absolute path to write the exported session to.
    pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SessionFormat {
    Chlsj,
    Har,
    Chls,
    Xml,
    Csv,
}

impl SessionFormat {
    pub fn ext(self) -> &'static str {
        match self {
            SessionFormat::Chlsj => "chlsj",
            SessionFormat::Har => "har",
            SessionFormat::Chls => "chls",
            SessionFormat::Xml => "xml",
            SessionFormat::Csv => "csv",
        }
    }
}
