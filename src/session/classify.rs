use super::Transaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceClass {
    Control,
    ConnectTunnel,
    StaticAsset,
    Font,
    Media,
    Document,
    Script,
    ApiCandidate,
    Unknown,
}

impl ResourceClass {
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceClass::Control => "control",
            ResourceClass::ConnectTunnel => "connect_tunnel",
            ResourceClass::StaticAsset => "static_asset",
            ResourceClass::Font => "font",
            ResourceClass::Media => "media",
            ResourceClass::Document => "document",
            ResourceClass::Script => "script",
            ResourceClass::ApiCandidate => "api_candidate",
            ResourceClass::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Classification {
    pub class: ResourceClass,
    pub priority: i64,
    pub reasons: Vec<&'static str>,
}

const STATIC_EXTS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".ico", ".css", ".map",
];
const FONT_EXTS: &[&str] = &[".woff", ".woff2", ".ttf", ".eot", ".otf"];
const MEDIA_EXTS: &[&str] = &[".mp3", ".mp4", ".wav", ".webm", ".mov", ".avi", ".m4a"];
const SCRIPT_EXTS: &[&str] = &[".js", ".mjs"];
const API_HINTS: &[&str] = &[
    "/api/", "/graphql", "/rpc", "/auth", "/login", "/token", "/session",
];

pub fn classify(t: &Transaction) -> Classification {
    let host = t.host.to_ascii_lowercase();
    let method = t.method.to_ascii_uppercase();
    let response_mime = t.mime.as_deref().unwrap_or("").to_ascii_lowercase();
    let request_mime = t
        .request
        .raw
        .content_type
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();

    let path_no_query = t.path.split('?').next().unwrap_or(&t.path);
    let ext = std::path::Path::new(path_no_query)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e.to_ascii_lowercase()))
        .unwrap_or_default();
    let ext = ext.as_str();

    if host == "control.charles" {
        return Classification {
            class: ResourceClass::Control,
            priority: 0,
            reasons: Vec::new(),
        };
    }
    if method == "CONNECT" || t.tunnel {
        return Classification {
            class: ResourceClass::ConnectTunnel,
            priority: 0,
            reasons: Vec::new(),
        };
    }
    if response_mime.starts_with("image/") || STATIC_EXTS.contains(&ext) {
        return Classification {
            class: ResourceClass::StaticAsset,
            priority: 5,
            reasons: Vec::new(),
        };
    }
    if response_mime.starts_with("font/") || FONT_EXTS.contains(&ext) {
        return Classification {
            class: ResourceClass::Font,
            priority: 5,
            reasons: Vec::new(),
        };
    }
    if response_mime.starts_with("audio/")
        || response_mime.starts_with("video/")
        || MEDIA_EXTS.contains(&ext)
    {
        return Classification {
            class: ResourceClass::Media,
            priority: 5,
            reasons: Vec::new(),
        };
    }
    if response_mime.contains("text/html") {
        return Classification {
            class: ResourceClass::Document,
            priority: 40,
            reasons: Vec::new(),
        };
    }
    if response_mime.contains("javascript")
        || response_mime.contains("ecmascript")
        || SCRIPT_EXTS.contains(&ext)
    {
        return Classification {
            class: ResourceClass::Script,
            priority: 35,
            reasons: Vec::new(),
        };
    }

    let mut reasons: Vec<&'static str> = Vec::new();
    let mut score: i64 = 20;
    let path_lower = t.path.to_ascii_lowercase();

    if response_mime.contains("application/json") || request_mime.contains("application/json") {
        score += 40;
        reasons.push("json_content_type");
    }
    if API_HINTS.iter().any(|hint| path_lower.contains(hint)) {
        score += 25;
        reasons.push("api_path_hint");
    }
    if matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        score += 15;
        reasons.push("mutating_method");
    }
    if matches!(t.status, Some(s) if s >= 400) {
        score += 20;
        reasons.push("error_status");
    }

    if !reasons.is_empty() {
        Classification {
            class: ResourceClass::ApiCandidate,
            priority: score,
            reasons,
        }
    } else {
        Classification {
            class: ResourceClass::Unknown,
            priority: 20,
            reasons: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{HttpMessage, RawBody};

    fn json_request() -> HttpMessage {
        HttpMessage {
            raw: RawBody {
                content_type: Some("application/json".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn control_charles_is_control() {
        let t = Transaction {
            host: "control.charles".into(),
            method: "GET".into(),
            path: "/".into(),
            ..Default::default()
        };
        let c = classify(&t);
        assert_eq!(c.class, ResourceClass::Control);
        assert_eq!(c.priority, 0);
        assert!(c.reasons.is_empty());
    }

    #[test]
    fn connect_tunnel_via_flag() {
        let t = Transaction {
            host: "secure.example.com".into(),
            method: "GET".into(),
            path: "/".into(),
            tunnel: true,
            ..Default::default()
        };
        let c = classify(&t);
        assert_eq!(c.class, ResourceClass::ConnectTunnel);
        assert_eq!(c.priority, 0);
    }

    #[test]
    fn png_image_is_static_asset() {
        let t = Transaction {
            host: "cdn.example.com".into(),
            method: "GET".into(),
            path: "/assets/logo.png?v=2".into(),
            mime: Some("image/png".into()),
            ..Default::default()
        };
        let c = classify(&t);
        assert_eq!(c.class, ResourceClass::StaticAsset);
        assert_eq!(c.priority, 5);
    }

    #[test]
    fn html_is_document() {
        let t = Transaction {
            host: "example.com".into(),
            method: "GET".into(),
            path: "/index".into(),
            mime: Some("text/html".into()),
            ..Default::default()
        };
        let c = classify(&t);
        assert_eq!(c.class, ResourceClass::Document);
        assert_eq!(c.priority, 40);
    }

    #[test]
    fn js_extension_is_script() {
        let t = Transaction {
            host: "example.com".into(),
            method: "GET".into(),
            path: "/static/app.js".into(),
            ..Default::default()
        };
        let c = classify(&t);
        assert_eq!(c.class, ResourceClass::Script);
        assert_eq!(c.priority, 35);
    }

    #[test]
    fn mutating_json_error_is_api_candidate_score_120() {
        let t = Transaction {
            host: "api.example.com".into(),
            method: "POST".into(),
            path: "/api/v1/login".into(),
            status: Some(500),
            mime: Some("application/json".into()),
            request: json_request(),
            ..Default::default()
        };
        let c = classify(&t);
        assert_eq!(c.class, ResourceClass::ApiCandidate);
        assert_eq!(c.priority, 120);
        assert_eq!(
            c.reasons,
            vec![
                "json_content_type",
                "api_path_hint",
                "mutating_method",
                "error_status",
            ]
        );
    }

    #[test]
    fn plain_get_text_is_unknown() {
        let t = Transaction {
            host: "example.com".into(),
            method: "GET".into(),
            path: "/healthz".into(),
            status: Some(200),
            mime: Some("text/plain".into()),
            ..Default::default()
        };
        let c = classify(&t);
        assert_eq!(c.class, ResourceClass::Unknown);
        assert_eq!(c.priority, 20);
        assert!(c.reasons.is_empty());
    }
}
