//! Documented `control.charles` control verbs (recording, throttling, tools).
//!
//! These are the stable, publicly-known endpoints. Each is an HTTP GET whose
//! success we infer from a 2xx response.

use super::WebClient;
use crate::error::CharlesError;

/// The toggleable Charles tools, with their Web Interface path segment.
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
    /// Path segment Charles uses under `/tools/<segment>/`.
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

    /// Enable (optionally with a preset) or disable bandwidth throttling.
    pub async fn set_throttling(
        &self,
        enabled: bool,
        preset: Option<&str>,
    ) -> Result<(), CharlesError> {
        let path = if enabled {
            match preset {
                Some(p) => {
                    // form-urlencoding matches Charles's `?preset=56+kbps+Modem`.
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

    /// Returns true if the tool reports as enabled, parsed from the status HTML.
    pub async fn get_tool_status(&self, tool: CharlesTool) -> Result<bool, CharlesError> {
        let html = self
            .get_control_text(&format!("tools/{}/", tool.segment()))
            .await?;
        let low = html.to_lowercase();
        // "enabled" is not a substring of "disabled", so order-independent checks are safe.
        if low.contains("disabled") {
            Ok(false)
        } else if low.contains("enabled") {
            Ok(true)
        } else {
            Err(CharlesError::Parse(
                "could not find an enabled/disabled status in the tool page".into(),
            ))
        }
    }
}
