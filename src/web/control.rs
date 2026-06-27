use scraper::{Html, Selector};

use super::WebClient;
use crate::error::CharlesError;

#[derive(Debug, Clone)]
pub struct ThrottleInfo {
    pub active: bool,
    pub presets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharlesTool {
    Breakpoints,
    NoCaching,
    BlockCookies,
    MapRemote,
    MapLocal,
    Rewrite,
    BlockList,
    AllowList,
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
            CharlesTool::BlockList => "block-list",
            CharlesTool::AllowList => "allow-list",
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

    pub async fn throttle_info(&self) -> Result<ThrottleInfo, CharlesError> {
        let html = self.get_control_text("throttling/").await?;
        Ok(ThrottleInfo {
            active: throttle_active(&html),
            presets: scrape_presets(&html),
        })
    }

    pub async fn set_throttling(
        &self,
        enabled: bool,
        preset: Option<&str>,
    ) -> Result<(), CharlesError> {
        let path = if enabled {
            match preset {
                Some(p) => {
                    let info = self.throttle_info().await?;
                    let Some(name) = info.presets.iter().find(|n| n.eq_ignore_ascii_case(p)) else {
                        return Err(CharlesError::InvalidArg(format!(
                            "unknown throttling preset {p:?}; configured presets are: {}",
                            info.presets.join(", ")
                        )));
                    };
                    let q = url::form_urlencoded::Serializer::new(String::new())
                        .append_pair("preset", name)
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

fn throttle_active(html: &str) -> bool {
    let low = html.to_lowercase();
    match low.split_once("status:") {
        Some((_, rest)) => {
            let window: String = rest.chars().take(40).collect();
            !window.contains("stopped")
        }
        None => false,
    }
}

fn scrape_presets(html: &str) -> Vec<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse(r#"select[name="preset"] option"#).unwrap();
    let mut out = Vec::new();
    for opt in doc.select(&sel) {
        let raw = opt
            .value()
            .attr("value")
            .map(str::to_string)
            .unwrap_or_else(|| opt.text().collect::<String>());
        let v = decode_entities(raw.trim());
        if !v.is_empty() {
            out.push(v);
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&#x2F;", "/")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_match_the_real_tool_index() {
        let expected = [
            (CharlesTool::Breakpoints, "breakpoints"),
            (CharlesTool::NoCaching, "no-caching"),
            (CharlesTool::BlockCookies, "block-cookies"),
            (CharlesTool::MapRemote, "map-remote"),
            (CharlesTool::MapLocal, "map-local"),
            (CharlesTool::Rewrite, "rewrite"),
            (CharlesTool::BlockList, "block-list"),
            (CharlesTool::AllowList, "allow-list"),
            (CharlesTool::DnsSpoofing, "dns-spoofing"),
            (CharlesTool::AutoSave, "auto-save"),
            (CharlesTool::ClientProcess, "client-process"),
        ];
        for (tool, seg) in expected {
            assert_eq!(tool.segment(), seg);
        }
    }

    #[test]
    fn scrapes_presets_and_status_from_the_real_page() {
        let html = include_str!("../../tests/fixtures/throttling_page.html");
        let presets = scrape_presets(html);
        assert!(presets.iter().any(|p| p == "3G"));
        assert!(presets.iter().any(|p| p == "4G"));
        assert!(
            presets.iter().any(|p| p == "256 kbps ISDN/DSL"),
            "the &#x2F; entity must be decoded, got {presets:?}"
        );
        assert!(
            !presets.iter().any(|p| p.is_empty()),
            "the empty Current option is skipped"
        );
        assert!(!throttle_active(html), "fixture shows Throttling Stopped");
    }
}
