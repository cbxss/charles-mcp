//! The MCP server: rmcp tool wiring over the Charles Web Interface client.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};

use std::path::Path;

use regex::Regex;

use crate::config::Config;
use crate::error::CharlesError;
use crate::format;
use crate::session::{Body, Session, body, convert};
use crate::tools::inspect::{self, ListFilters, Matcher};
use crate::tools::{
    ConfirmReq, ExportReq, GetRequestReq, ListRequestsReq, ReadFileReq, SearchReq, SetToolReq,
    StatsReq, ThrottlingReq, ToolNameReq,
};
use crate::web::WebClient;

#[derive(Clone)]
pub struct CharlesServer {
    web: Arc<WebClient>,
}

impl CharlesServer {
    pub fn new(cfg: Arc<Config>) -> Result<Self, CharlesError> {
        Ok(Self {
            web: Arc::new(WebClient::new(cfg)?),
        })
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
        description = "Enable or disable bandwidth throttling, optionally selecting a preset \
                       (e.g. 3G, 4G, \"56 kbps Modem\")."
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
        description = "Enable or disable a Charles tool: breakpoints, no-caching, block-cookies, \
                       map-remote, map-local, rewrite, black-list, white-list, dns-spoofing, \
                       auto-save, client-process."
    )]
    async fn set_tool(
        &self,
        Parameters(req): Parameters<SetToolReq>,
    ) -> Result<CallToolResult, ErrorData> {
        self.web.set_tool(req.tool.to_web(), req.enabled).await?;
        Ok(Self::ok(format!(
            "Tool {:?} {}.",
            req.tool,
            if req.enabled { "enabled" } else { "disabled" }
        )))
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
        let session = convert::read_session_file(self.web.config(), Path::new(&req.path)).await?;
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
        let req_body = body::decode(&t.request.raw, max);
        let resp_body = t
            .response
            .as_ref()
            .map(|r| body::decode(&r.raw, max))
            .unwrap_or(Body::NotCaptured);
        Ok(Self::ok(format::transaction_detail(
            t, &req_body, &resp_body,
        )))
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
        let bytes = self.web.fetch_session_in_format(req.format.ext()).await?;
        tokio::fs::write(&req.path, &bytes)
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
