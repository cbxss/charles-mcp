pub mod inspect;

use std::collections::HashMap;

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::web::control::CharlesTool;

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
    pub enabled: bool,
    #[serde(default)]
    pub preset: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetToolReq {
    pub tool: ToolName,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ToolNameReq {
    pub tool: ToolName,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConfirmReq {
    pub confirm: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResetReq {
    pub confirm: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileReq {
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRequestsReq {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub path_regex: Option<String>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub resource_class: Option<String>,
    #[serde(default)]
    pub min_priority: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetRequestReq {
    pub index: usize,
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
    #[serde(default)]
    pub proto_type: Option<String>,
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WsMessagesReq {
    pub index: usize,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
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
    pub query: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub fields: Option<Vec<SearchField>>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StatsReq {
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReplayReq {
    pub index: usize,
    pub confirm: bool,
    #[serde(default)]
    pub allow_mutating: bool,
    #[serde(default)]
    pub query_overrides: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub header_overrides: Option<HashMap<String, Option<String>>>,
    #[serde(default)]
    pub json_overrides: Option<serde_json::Value>,
    #[serde(default)]
    pub body_text: Option<String>,
    #[serde(default)]
    pub use_proxy: bool,
    #[serde(default)]
    pub follow_redirects: bool,
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
    #[serde(default)]
    pub file_path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportReq {
    pub format: SessionFormat,
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
