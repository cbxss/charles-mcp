//! The MCP server: rmcp tool wiring over the Charles Web Interface client.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};

use std::path::{Path, PathBuf};

use regex::Regex;

use crate::config::Config;
use crate::error::CharlesError;
use crate::format;
use crate::session::{Body, Session, WsDirection, WsOpcode, body, convert};
use crate::tools::inspect::{self, ListFilters, Matcher};
use crate::tools::{
    ConfirmReq, ExportReq, GetRequestReq, ListRequestsReq, ReadFileReq, SearchReq, SetToolReq,
    StatsReq, ThrottlingReq, ToolName, ToolNameReq, WsMessagesReq,
};
use crate::web::WebClient;

/// Validate a caller-supplied session path: it must be absolute (kills `-`
/// argv-injection and relative surprises) and end in one of `allowed_exts`
/// (so an agent can't be steered into reading/overwriting `~/.zshrc`, a plist,
/// `authorized_keys`, etc.).
fn validate_session_path(path: &str, allowed_exts: &[&str]) -> Result<PathBuf, CharlesError> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(CharlesError::InvalidArg(format!(
            "path must be absolute, got '{path}'"
        )));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !allowed_exts.contains(&ext.as_str()) {
        return Err(CharlesError::InvalidArg(format!(
            "path must end in one of {allowed_exts:?}, got '{path}'"
        )));
    }
    Ok(p.to_path_buf())
}

#[derive(Clone)]
pub struct CharlesServer {
    web: Arc<WebClient>,
    /// Loaded `.proto` descriptors (from --proto-dir) for named decoding.
    #[cfg(feature = "proto")]
    proto: Arc<Option<crate::session::protobuf::ProtoPool>>,
}

#[cfg(feature = "proto")]
fn load_proto_pool(cfg: &Config) -> Option<crate::session::protobuf::ProtoPool> {
    let dir = cfg.proto_dir.as_ref()?;
    match crate::session::protobuf::ProtoPool::load_dir(dir) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("failed to load --proto-dir {}: {e}", dir.display());
            None
        }
    }
}

impl CharlesServer {
    pub fn new(cfg: Arc<Config>) -> Result<Self, CharlesError> {
        #[cfg(feature = "proto")]
        let proto = Arc::new(load_proto_pool(&cfg));
        let web = Arc::new(WebClient::new(cfg)?);
        Ok(Self {
            web,
            #[cfg(feature = "proto")]
            proto,
        })
    }

    fn decode_opts<'a>(
        &'a self,
        proto_type: Option<&'a str>,
    ) -> crate::session::body::DecodeOptions<'a> {
        crate::session::body::DecodeOptions {
            #[cfg(feature = "proto")]
            pool: self.proto.as_ref().as_ref(),
            proto_type,
        }
    }

    fn ok(msg: impl Into<String>) -> CallToolResult {
        CallToolResult::success(vec![Content::text(msg.into())])
    }

    /// Resolve a session from a file path, or the live Charles session if none.
    async fn resolve_session(&self, file_path: Option<&str>) -> Result<Session, CharlesError> {
        match file_path {
            Some(p) => convert::read_session_file(self.web.config(), Path::new(p)).await,
            None => self.web.fetch_live_session().await,
        }
    }
}

#[tool_router]
impl CharlesServer {
    #[tool(
        description = "Check connectivity to the Charles Web Interface and report the proxy, \
                       whether it is reachable/authenticated, and whether the Charles binary is \
                       available for offline conversion. Run this first."
    )]
    async fn charles_status(&self) -> Result<CallToolResult, ErrorData> {
        let r = self.web.status().await;
        let text = format!(
            "Charles status\n\
             - proxy: {}\n\
             - control host: {}\n\
             - reachable: {}\n\
             - authenticated: {}\n\
             - charles binary present: {}\n\
             - {}",
            r.proxy, r.control_host, r.reachable, r.authenticated, r.charles_bin_present, r.note
        );
        Ok(Self::ok(text))
    }

    #[tool(description = "Start recording traffic in Charles.")]
    async fn start_recording(&self) -> Result<CallToolResult, ErrorData> {
        self.web.start_recording().await?;
        Ok(Self::ok("Recording started."))
    }

    #[tool(description = "Stop recording traffic in Charles.")]
    async fn stop_recording(&self) -> Result<CallToolResult, ErrorData> {
        self.web.stop_recording().await?;
        Ok(Self::ok("Recording stopped."))
    }

    #[tool(
        description = "Enable or disable bandwidth throttling, optionally selecting a preset. The \
                       preset must exactly match a name configured in Charles (Proxy → Throttle \
                       Settings; defaults include \"3G\", \"4G\", \"56 kbps Modem\"). Charles \
                       silently ignores an unknown preset, and this server can't verify it took \
                       effect — confirm in the Charles UI if unsure."
    )]
    async fn set_throttling(
        &self,
        Parameters(req): Parameters<ThrottlingReq>,
    ) -> Result<CallToolResult, ErrorData> {
        self.web
            .set_throttling(req.enabled, req.preset.as_deref())
            .await?;
        let msg = match (req.enabled, req.preset) {
            (true, Some(p)) => format!("Throttling enabled with preset '{p}'."),
            (true, None) => "Throttling enabled.".to_string(),
            (false, _) => "Throttling disabled.".to_string(),
        };
        Ok(Self::ok(msg))
    }

    #[tool(
        description = "Enable/disable a Charles tool's master switch: breakpoints, no-caching, \
                       block-cookies, map-remote, map-local, rewrite, black-list, white-list, \
                       dns-spoofing, auto-save, client-process. NOTE: the rule-based tools \
                       (map-*, rewrite, *-list, dns-spoofing) do nothing without rules configured \
                       in the Charles GUI — this server cannot manage rules — and breakpoints will \
                       pause/hang matching traffic. See the per-call notes."
    )]
    async fn set_tool(
        &self,
        Parameters(req): Parameters<SetToolReq>,
    ) -> Result<CallToolResult, ErrorData> {
        self.web.set_tool(req.tool.to_web(), req.enabled).await?;
        let mut msg = format!(
            "Tool {:?} {}.",
            req.tool,
            if req.enabled { "enabled" } else { "disabled" }
        );
        if req.enabled {
            match req.tool {
                ToolName::Breakpoints => msg.push_str(
                    " ⚠ Charles will now PAUSE matching requests in its GUI waiting for manual \
                     Edit/Execute/Abort. This server cannot respond to breakpoints, so live \
                     traffic may hang until you act in Charles or disable this.",
                ),
                ToolName::MapRemote
                | ToolName::MapLocal
                | ToolName::Rewrite
                | ToolName::BlackList
                | ToolName::WhiteList
                | ToolName::DnsSpoofing => msg.push_str(
                    " Note: this only affects traffic if matching rules exist (configured in the \
                     Charles GUI — this server can't add rules). With no rules it's a no-op.",
                ),
                _ => {}
            }
        }
        Ok(Self::ok(msg))
    }

    #[tool(description = "Report whether a Charles tool is currently enabled or disabled.")]
    async fn get_tool_status(
        &self,
        Parameters(req): Parameters<ToolNameReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let enabled = self.web.get_tool_status(req.tool.to_web()).await?;
        Ok(Self::ok(format!(
            "Tool {:?} is {}.",
            req.tool,
            if enabled { "enabled" } else { "disabled" }
        )))
    }

    #[tool(
        description = "Parse a Charles session file (.chls, .har, or .chlsj) from disk and list \
                       its requests as a compact table. A .chls file is converted via the Charles \
                       binary first."
    )]
    async fn read_session_file(
        &self,
        Parameters(req): Parameters<ReadFileReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let path = validate_session_path(&req.path, &["chls", "chlz", "har", "chlsj"])?;
        let session = convert::read_session_file(self.web.config(), &path).await?;
        let summaries = session.summaries();
        let table = format::summary_table(&summaries);
        Ok(Self::ok(format!(
            "{} request(s) in {}\n\n{}",
            summaries.len(),
            req.path,
            table
        )))
    }

    #[tool(
        description = "Browse/filter captured requests by host, method, status, path_regex, or \
                       mime -> a compact table of indices. Use search_traffic for full-text/body \
                       search; get_request for one request's full detail. Live session unless \
                       file_path (.har/.chlsj/.chls) is given."
    )]
    async fn list_requests(
        &self,
        Parameters(req): Parameters<ListRequestsReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.resolve_session(req.file_path.as_deref()).await?;
        let path_regex = match req.path_regex.as_deref() {
            Some(p) => Some(
                Regex::new(p)
                    .map_err(|e| CharlesError::InvalidArg(format!("bad path_regex: {e}")))?,
            ),
            None => None,
        };
        let filters = ListFilters {
            host: req.host.as_deref(),
            method: req.method.as_deref(),
            status: req.status,
            path_regex,
            mime: req.mime.as_deref(),
            limit: req.limit.unwrap_or(50),
        };
        let result = inspect::list(&session, &filters);
        let truncated = result.total > result.rows.len();
        let header = format!(
            "requests: {} total, {} shown{}. Pass a row's # to get_request.\n",
            result.total,
            result.rows.len(),
            if truncated {
                " (truncated; raise limit)"
            } else {
                ""
            },
        );
        Ok(Self::ok(format!(
            "{header}{}",
            format::summary_table(&result.rows)
        )))
    }

    #[tool(
        description = "Fetch ONE request by index (from list_requests/search_traffic): full \
                       headers, decoded/pretty bodies, and timing. Use max_body_bytes to cap \
                       large bodies."
    )]
    async fn get_request(
        &self,
        Parameters(req): Parameters<GetRequestReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.resolve_session(req.file_path.as_deref()).await?;
        let count = session.transactions.len();
        let Some(t) = session.get(req.index) else {
            // Return as a visible tool error (not a protocol error) so the agent
            // sees the valid range and can recover.
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "request index {} is out of range; this session has {count} request(s) \
                 (valid indices 0..{}). Call list_requests first.",
                req.index,
                count.saturating_sub(1),
            ))]));
        };
        let max = req
            .max_body_bytes
            .unwrap_or(self.web.config().body_max_bytes);
        let opts = self.decode_opts(req.proto_type.as_deref());
        let req_body = body::decode_with(&t.request.raw, max, &opts);
        let resp_body = t
            .response
            .as_ref()
            .map(|r| body::decode_with(&r.raw, max, &opts))
            .unwrap_or(Body::NotCaptured);
        Ok(Self::ok(format::transaction_detail(
            t, &req_body, &resp_body,
        )))
    }

    #[tool(
        description = "List the decoded WebSocket frames of a wss/ws transaction (by index): \
                       direction (→ sent / ← received), opcode, and decoded payload (text/JSON, \
                       or protobuf for binary frames). Filter by direction; cap with limit."
    )]
    async fn get_websocket_messages(
        &self,
        Parameters(req): Parameters<WsMessagesReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.resolve_session(req.file_path.as_deref()).await?;
        let Some(t) = session.get(req.index) else {
            return Ok(CallToolResult::error(vec![Content::text(format!(
                "request index {} is out of range.",
                req.index
            ))]));
        };
        let Some(frames) = &t.websocket else {
            return Ok(Self::ok(format!(
                "request #{} is not a WebSocket connection (no frames).",
                req.index
            )));
        };
        let max = req
            .max_body_bytes
            .unwrap_or(self.web.config().body_max_bytes);
        let limit = req.limit.unwrap_or(100);
        let want = req.direction.as_deref().map(|d| d.to_ascii_lowercase());
        let opts = self.decode_opts(None);

        let mut out = format!("{} WebSocket frame(s) in #{}\n\n", frames.len(), req.index);
        let mut shown = 0usize;
        for (i, m) in frames.iter().enumerate() {
            let dir = match m.direction {
                WsDirection::Sent => "sent",
                WsDirection::Received => "received",
            };
            if want.as_deref().is_some_and(|w| w != dir) {
                continue;
            }
            if shown >= limit {
                out.push_str(&format!(
                    "… ({} more frames; raise limit)\n",
                    frames.len() - i
                ));
                break;
            }
            shown += 1;
            let arrow = if dir == "sent" { "→" } else { "←" };
            out.push_str(&format!("[{i}] {arrow} {:?}\n", m.opcode));
            // Binary WS frames are often protobuf (MQTT/Tesla signaling, etc.) —
            // try a schemaless decode before falling back to text/hex.
            if matches!(m.opcode, WsOpcode::Binary)
                && let Some((tree, _)) =
                    crate::session::protobuf::try_decode(&m.payload.bytes, &opts)
            {
                out.push_str("(protobuf, schemaless)\n");
                out.push_str(&tree);
                if !tree.ends_with('\n') {
                    out.push('\n');
                }
            } else {
                out.push_str(&format::render_body_str(&body::decode_with(
                    &m.payload, max, &opts,
                )));
            }
        }
        Ok(Self::ok(out))
    }

    #[tool(
        description = "Full-text/regex search across request URLs, headers, and bodies -> matching \
                       indices with snippets. Substring (case-insensitive) unless regex=true. Use \
                       list_requests to filter by structured fields; get_request to read a match."
    )]
    async fn search_traffic(
        &self,
        Parameters(req): Parameters<SearchReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.resolve_session(req.file_path.as_deref()).await?;
        let matcher = Matcher::build(&req.query, req.regex)?;
        let fields = req.fields.unwrap_or_default();
        let limit = req.limit.unwrap_or(50);
        let hits = inspect::search(&session, &matcher, &fields, limit);
        let capped = if hits.len() >= limit {
            format!(" (capped at limit={limit}; narrow the query or raise limit)")
        } else {
            String::new()
        };
        Ok(Self::ok(format!(
            "search {:?}: {} hit(s){capped}. Pass a #'s index to get_request.\n\n{}",
            req.query,
            hits.len(),
            format::search_results(&hits)
        )))
    }

    #[tool(
        description = "Aggregate statistics for the session: counts by host/status/mime, total \
                       response bytes, error count, and the slowest requests."
    )]
    async fn get_session_stats(
        &self,
        Parameters(req): Parameters<StatsReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let session = self.resolve_session(req.file_path.as_deref()).await?;
        Ok(Self::ok(format::stats_report(&inspect::stats(&session))))
    }

    #[tool(
        description = "Export the current live Charles session to a file in the given format \
                       (chlsj, har, chls, xml, or csv)."
    )]
    async fn export_session(
        &self,
        Parameters(req): Parameters<ExportReq>,
    ) -> Result<CallToolResult, ErrorData> {
        let path = validate_session_path(&req.path, &[req.format.ext()])?;
        let bytes = self.web.fetch_session_in_format(req.format.ext()).await?;
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(CharlesError::Io)?;
        Ok(Self::ok(format!(
            "Exported {} bytes ({}) to {}",
            bytes.len(),
            req.format.ext(),
            req.path
        )))
    }

    #[tool(description = "Clear the current Charles session. Destructive — requires confirm=true.")]
    async fn clear_session(
        &self,
        Parameters(req): Parameters<ConfirmReq>,
    ) -> Result<CallToolResult, ErrorData> {
        if !req.confirm {
            return Err(
                CharlesError::InvalidArg("set confirm=true to clear the session".into()).into(),
            );
        }
        self.web.clear_session().await?;
        Ok(Self::ok("Session cleared."))
    }

    #[tool(description = "Quit Charles. Destructive — requires confirm=true.")]
    async fn quit_charles(
        &self,
        Parameters(req): Parameters<ConfirmReq>,
    ) -> Result<CallToolResult, ErrorData> {
        if !req.confirm {
            return Err(CharlesError::InvalidArg("set confirm=true to quit Charles".into()).into());
        }
        self.web.quit_charles().await?;
        Ok(Self::ok("Quit signal sent to Charles."))
    }
}

#[tool_handler]
impl ServerHandler for CharlesServer {
    fn get_info(&self) -> ServerInfo {
        // ServerInfo/Implementation are #[non_exhaustive]; build from Default.
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("charles-mcp", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(
            "Control Charles Proxy 5 and inspect captured HTTP(S) traffic. \
             Call charles_status first to verify the connection."
                .into(),
        );
        info
    }
}

#[cfg(test)]
mod tests {
    use super::validate_session_path;

    #[test]
    fn path_must_be_absolute_with_allowed_ext() {
        // relative path → rejected (also blocks leading-dash argv injection)
        assert!(validate_session_path("relative.har", &["har"]).is_err());
        assert!(validate_session_path("-rf.har", &["har"]).is_err());
        // absolute but disallowed extension → rejected (no overwriting ~/.zshrc)
        assert!(validate_session_path("/home/u/.zshrc", &["har", "chlsj"]).is_err());
        // absolute with an allowed extension → ok
        assert!(validate_session_path("/tmp/s.har", &["har", "chlsj", "chls"]).is_ok());
        assert!(validate_session_path("/tmp/s.chlsj", &["chlsj"]).is_ok());
    }
}
