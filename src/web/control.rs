use super::WebClient;
use crate::error::CharlesError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharlesTool {
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

impl CharlesTool {
    pub fn segment(self) -> &'static str {
        match self {
            CharlesTool::Breakpoints => "breakpoints",
            CharlesTool::NoCaching => "no-caching",
            CharlesTool::BlockCookies => "block-cookies",
            CharlesTool::MapRemote => "map-remote",
            CharlesTool::MapLocal => "map-local",
            CharlesTool::Rewrite => "rewrite",
            CharlesTool::BlackList => "black-list",
            CharlesTool::WhiteList => "white-list",
            CharlesTool::DnsSpoofing => "dns-spoofing",
            CharlesTool::AutoSave => "auto-save",
            CharlesTool::ClientProcess => "client-process",
        }
    }
}

impl WebClient {
    pub async fn start_recording(&self) -> Result<(), CharlesError> {
        self.get_control_text("recording/start").await.map(drop)
    }

    pub async fn stop_recording(&self) -> Result<(), CharlesError> {
        self.get_control_text("recording/stop").await.map(drop)
    }

    pub async fn set_throttling(
        &self,
        enabled: bool,
        preset: Option<&str>,
    ) -> Result<(), CharlesError> {
        let path = if enabled {
            match preset {
                Some(p) => {
                    let q = url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("preset", p)
                        .finish();
                    format!("throttling/activate?{q}")
                }
                None => "throttling/activate".to_string(),
            }
        } else {
            "throttling/deactivate".to_string()
        };
        self.get_control_text(&path).await.map(drop)
    }

    pub async fn set_tool(&self, tool: CharlesTool, enabled: bool) -> Result<(), CharlesError> {
        let verb = if enabled { "enable" } else { "disable" };
        self.get_control_text(&format!("tools/{}/{}", tool.segment(), verb))
            .await
            .map(drop)
    }

    pub async fn get_tool_status(&self, tool: CharlesTool) -> Result<bool, CharlesError> {
        let html = self
            .get_control_text(&format!("tools/{}/", tool.segment()))
            .await?;
        let low = html.to_lowercase();
        let after = low
            .split_once("status:")
            .map(|(_, rest)| rest)
            .ok_or_else(|| {
                CharlesError::Parse("no 'Status:' marker found on the tool page".into())
            })?;
        let window: String = after.chars().take(40).collect();
        if window.contains("disabled") {
            Ok(false)
        } else if window.contains("enabled") {
            Ok(true)
        } else {
            Err(CharlesError::Parse(
                "the tool page's 'Status:' marker had no enabled/disabled value".into(),
            ))
        }
    }
}
