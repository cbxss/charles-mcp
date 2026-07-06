use std::path::{Path, PathBuf};

use rmcp::schemars::{self, JsonSchema};
use serde::Deserialize;

use crate::error::CharlesError;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteRulesReq {
    pub path: String,
    #[serde(default)]
    pub enable_tools: bool,
    #[serde(default)]
    pub save_to_charles_config: bool,
    #[serde(default)]
    pub config_path: Option<String>,
    #[serde(default)]
    pub confirm: bool,
    #[serde(default)]
    pub map_local: Vec<MapLocalRuleReq>,
    #[serde(default)]
    pub map_remote: Vec<MapRemoteRuleReq>,
    #[serde(default)]
    pub rewrite_sets: Vec<RewriteSetReq>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LocationReq {
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MapLocalRuleReq {
    pub from: LocationReq,
    pub local_path: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub case_sensitive: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MapRemoteRuleReq {
    pub from: LocationReq,
    pub to: LocationReq,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub preserve_host_header: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RewriteSetReq {
    pub name: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub locations: Vec<LocationReq>,
    #[serde(default)]
    pub rules: Vec<RewriteRuleReq>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RewriteRuleKind {
    RequestHeader,
    ResponseHeader,
    Url,
    Body,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RewriteRuleReq {
    #[serde(default)]
    pub kind: Option<RewriteRuleKind>,
    #[serde(default)]
    pub rule_type: Option<u8>,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub match_header: Option<String>,
    #[serde(default)]
    pub match_value: Option<String>,
    #[serde(default)]
    pub match_header_regex: bool,
    #[serde(default)]
    pub match_value_regex: bool,
    #[serde(default)]
    pub match_request: Option<bool>,
    #[serde(default)]
    pub match_response: Option<bool>,
    #[serde(default)]
    pub new_header: Option<String>,
    #[serde(default)]
    pub new_value: Option<String>,
    #[serde(default)]
    pub new_header_regex: bool,
    #[serde(default)]
    pub new_value_regex: bool,
    #[serde(default)]
    pub match_whole_value: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub replace_type: Option<u8>,
}

fn default_true() -> bool {
    true
}

pub fn validate_output_path(path: &str) -> Result<PathBuf, CharlesError> {
    validate_xml_path(path, "path")
}

pub fn validate_config_path(path: &str) -> Result<PathBuf, CharlesError> {
    validate_xml_path(path, "config_path")
}

fn validate_xml_path(path: &str, label: &str) -> Result<PathBuf, CharlesError> {
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(CharlesError::InvalidArg(format!(
            "{label} must be absolute, got '{path}'"
        )));
    }
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "xml" | "config") {
        return Err(CharlesError::InvalidArg(format!(
            "{label} must end in .xml or .config, got '{path}'"
        )));
    }
    Ok(p.to_path_buf())
}

pub fn merge_into_charles_config(existing: &str, req: &WriteRulesReq) -> String {
    let mut xml = if existing.trim().is_empty() {
        empty_config()
    } else {
        existing.to_string()
    };
    xml = ensure_tool_config(xml);
    if !req.map_local.is_empty() {
        xml = upsert_tool_entry(
            &xml,
            "Map Local",
            &tool_entry("Map Local", &build_map_local_at(&req.map_local, 4)),
        );
    }
    if !req.map_remote.is_empty() {
        xml = upsert_tool_entry(
            &xml,
            "Map Remote",
            &tool_entry("Map Remote", &build_map_remote_at(&req.map_remote, 4)),
        );
    }
    if !req.rewrite_sets.is_empty() {
        xml = upsert_tool_entry(
            &xml,
            "Rewrite",
            &tool_entry("Rewrite", &build_rewrite_at(&req.rewrite_sets, 4)),
        );
    }
    xml
}

pub fn build_rule_file(req: &WriteRulesReq) -> Result<String, CharlesError> {
    validate(req)?;
    Ok(build_settings(req))
}

fn validate(req: &WriteRulesReq) -> Result<(), CharlesError> {
    if req.map_local.is_empty() && req.map_remote.is_empty() && req.rewrite_sets.is_empty() {
        return Err(CharlesError::InvalidArg(
            "provide at least one map_local, map_remote, or rewrite_sets entry".into(),
        ));
    }
    for m in &req.map_local {
        require_any_location_field(&m.from, "map_local.from")?;
        require_absolute(&m.local_path, "map_local.local_path")?;
    }
    for m in &req.map_remote {
        require_any_location_field(&m.from, "map_remote.from")?;
        require_any_location_field(&m.to, "map_remote.to")?;
    }
    for set in &req.rewrite_sets {
        if set.name.trim().is_empty() {
            return Err(CharlesError::InvalidArg(
                "rewrite set name cannot be empty".into(),
            ));
        }
        if set.rules.is_empty() {
            return Err(CharlesError::InvalidArg(format!(
                "rewrite set {:?} must contain at least one rule",
                set.name
            )));
        }
        for loc in &set.locations {
            require_any_location_field(loc, "rewrite_sets.locations")?;
        }
        for rule in &set.rules {
            if rule.kind.is_none() && rule.rule_type.is_none() {
                return Err(CharlesError::InvalidArg(format!(
                    "rewrite set {:?} has a rule without kind or rule_type",
                    set.name
                )));
            }
        }
    }
    Ok(())
}

fn require_any_location_field(loc: &LocationReq, label: &str) -> Result<(), CharlesError> {
    let has_any = [
        loc.protocol.as_deref(),
        loc.host.as_deref(),
        loc.port.as_deref(),
        loc.path.as_deref(),
        loc.query.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|v| !v.trim().is_empty());
    if has_any {
        Ok(())
    } else {
        Err(CharlesError::InvalidArg(format!(
            "{label} must set at least one of protocol, host, port, path, or query"
        )))
    }
}

fn require_absolute(path: &str, label: &str) -> Result<(), CharlesError> {
    if Path::new(path).is_absolute() {
        Ok(())
    } else {
        Err(CharlesError::InvalidArg(format!(
            "{label} must be absolute, got '{path}'"
        )))
    }
}

fn build_settings(req: &WriteRulesReq) -> String {
    let mut out = header();
    out.push_str("<charles-export>\n");
    out.push_str("  <toolConfiguration>\n");
    out.push_str("    <configs>\n");
    tool_entry_into(
        &mut out,
        "Map Local",
        &build_map_local_at(&req.map_local, 4),
        3,
    );
    tool_entry_into(
        &mut out,
        "Map Remote",
        &build_map_remote_at(&req.map_remote, 4),
        3,
    );
    tool_entry_into(
        &mut out,
        "Rewrite",
        &build_rewrite_at(&req.rewrite_sets, 4),
        3,
    );
    out.push_str("    </configs>\n");
    out.push_str("  </toolConfiguration>\n");
    out.push_str("</charles-export>\n");
    out
}

fn build_map_local_at(mappings: &[MapLocalRuleReq], indent: usize) -> String {
    let mut out = String::new();
    pad(&mut out, indent);
    out.push_str("<mapLocal>\n");
    element_bool(&mut out, indent + 1, "toolEnabled", !mappings.is_empty());
    if mappings.is_empty() {
        pad(&mut out, indent + 1);
        out.push_str("<mappings />\n");
    } else {
        pad(&mut out, indent + 1);
        out.push_str("<mappings>\n");
        for mapping in mappings {
            pad(&mut out, indent + 2);
            out.push_str("<mapLocalMapping>\n");
            location_block(&mut out, indent + 3, "sourceLocation", &mapping.from);
            element(&mut out, indent + 3, "dest", &mapping.local_path);
            element_bool(&mut out, indent + 3, "enabled", mapping.enabled);
            element_bool(
                &mut out,
                indent + 3,
                "caseSensitive",
                mapping.case_sensitive,
            );
            pad(&mut out, indent + 2);
            out.push_str("</mapLocalMapping>\n");
        }
        pad(&mut out, indent + 1);
        out.push_str("</mappings>\n");
    }
    pad(&mut out, indent);
    out.push_str("</mapLocal>\n");
    out
}

fn build_map_remote_at(mappings: &[MapRemoteRuleReq], indent: usize) -> String {
    let mut out = String::new();
    pad(&mut out, indent);
    out.push_str("<map>\n");
    element_bool(&mut out, indent + 1, "toolEnabled", !mappings.is_empty());
    if mappings.is_empty() {
        pad(&mut out, indent + 1);
        out.push_str("<mappings />\n");
    } else {
        pad(&mut out, indent + 1);
        out.push_str("<mappings>\n");
        for mapping in mappings {
            pad(&mut out, indent + 2);
            out.push_str("<mapMapping>\n");
            location_block(&mut out, indent + 3, "sourceLocation", &mapping.from);
            location_block(&mut out, indent + 3, "destLocation", &mapping.to);
            element_bool(
                &mut out,
                indent + 3,
                "preserveHostHeader",
                mapping.preserve_host_header,
            );
            element_bool(&mut out, indent + 3, "enabled", mapping.enabled);
            pad(&mut out, indent + 2);
            out.push_str("</mapMapping>\n");
        }
        pad(&mut out, indent + 1);
        out.push_str("</mappings>\n");
    }
    pad(&mut out, indent);
    out.push_str("</map>\n");
    out
}

fn build_rewrite_at(sets: &[RewriteSetReq], indent: usize) -> String {
    let mut out = String::new();
    pad(&mut out, indent);
    out.push_str("<rewrite>\n");
    element_bool(&mut out, indent + 1, "toolEnabled", !sets.is_empty());
    element_bool(&mut out, indent + 1, "debugging", false);
    if sets.is_empty() {
        pad(&mut out, indent + 1);
        out.push_str("<sets />\n");
    } else {
        pad(&mut out, indent + 1);
        out.push_str("<sets>\n");
        for set in sets {
            rewrite_set(&mut out, set, indent + 2);
        }
        pad(&mut out, indent + 1);
        out.push_str("</sets>\n");
    }
    pad(&mut out, indent);
    out.push_str("</rewrite>\n");
    out
}

fn rewrite_set(out: &mut String, set: &RewriteSetReq, indent: usize) {
    pad(out, indent);
    out.push_str("<rewriteSet>\n");
    element_bool(out, indent + 1, "active", set.active);
    element(out, indent + 1, "name", &set.name);
    locations(out, indent + 1, "hosts", &set.locations);
    pad(out, indent + 1);
    out.push_str("<rules>\n");
    for rule in &set.rules {
        rewrite_rule(out, rule, indent + 2);
    }
    pad(out, indent + 1);
    out.push_str("</rules>\n");
    pad(out, indent);
    out.push_str("</rewriteSet>\n");
}

fn rewrite_rule(out: &mut String, rule: &RewriteRuleReq, indent: usize) {
    pad(out, indent);
    out.push_str("<rewriteRule>\n");
    element_bool(out, indent + 1, "active", rule.active);
    element_num(out, indent + 1, "ruleType", rewrite_rule_type(rule));
    element(
        out,
        indent + 1,
        "matchHeader",
        rule.match_header.as_deref().unwrap_or(""),
    );
    element(
        out,
        indent + 1,
        "matchValue",
        rule.match_value.as_deref().unwrap_or(""),
    );
    element_bool(out, indent + 1, "matchHeaderRegex", rule.match_header_regex);
    element_bool(out, indent + 1, "matchValueRegex", rule.match_value_regex);
    element_bool(
        out,
        indent + 1,
        "matchRequest",
        rule.match_request
            .unwrap_or_else(|| default_match_request(rule)),
    );
    element_bool(
        out,
        indent + 1,
        "matchResponse",
        rule.match_response
            .unwrap_or_else(|| default_match_response(rule)),
    );
    if let Some(v) = &rule.new_header {
        element(out, indent + 1, "newHeader", v);
    }
    if let Some(v) = &rule.new_value {
        element(out, indent + 1, "newValue", v);
    }
    element_bool(out, indent + 1, "newHeaderRegex", rule.new_header_regex);
    element_bool(out, indent + 1, "newValueRegex", rule.new_value_regex);
    element_bool(out, indent + 1, "matchWholeValue", rule.match_whole_value);
    element_bool(out, indent + 1, "caseSensitive", rule.case_sensitive);
    element_num(
        out,
        indent + 1,
        "replaceType",
        rule.replace_type.unwrap_or(2),
    );
    pad(out, indent);
    out.push_str("</rewriteRule>\n");
}

fn rewrite_rule_type(rule: &RewriteRuleReq) -> u8 {
    rule.rule_type
        .or_else(|| rule.kind.map(kind_rule_type))
        .expect("validated")
}

fn kind_rule_type(kind: RewriteRuleKind) -> u8 {
    match kind {
        RewriteRuleKind::RequestHeader => 1,
        RewriteRuleKind::ResponseHeader => 3,
        RewriteRuleKind::Url => 6,
        RewriteRuleKind::Body => 7,
    }
}

fn default_match_request(rule: &RewriteRuleReq) -> bool {
    matches!(
        rule.kind,
        Some(RewriteRuleKind::RequestHeader | RewriteRuleKind::Url | RewriteRuleKind::Body)
    )
}

fn default_match_response(rule: &RewriteRuleReq) -> bool {
    matches!(
        rule.kind,
        Some(RewriteRuleKind::ResponseHeader | RewriteRuleKind::Body)
    )
}

fn locations(out: &mut String, indent: usize, tag: &str, locs: &[LocationReq]) {
    pad(out, indent);
    out.push_str(&format!("<{tag}>\n"));
    if locs.is_empty() {
        pad(out, indent + 1);
        out.push_str("<locationPatterns />\n");
    } else {
        pad(out, indent + 1);
        out.push_str("<locationPatterns>\n");
        for loc in locs {
            pad(out, indent + 2);
            out.push_str("<locationMatch>\n");
            location_block(out, indent + 3, "location", loc);
            element_bool(out, indent + 3, "enabled", true);
            pad(out, indent + 2);
            out.push_str("</locationMatch>\n");
        }
        pad(out, indent + 1);
        out.push_str("</locationPatterns>\n");
    }
    pad(out, indent);
    out.push_str(&format!("</{tag}>\n"));
}

fn location_block(out: &mut String, indent: usize, tag: &str, loc: &LocationReq) {
    pad(out, indent);
    out.push_str(&format!("<{tag}>\n"));
    element_opt(out, indent + 1, "protocol", loc.protocol.as_deref());
    element_opt(out, indent + 1, "host", loc.host.as_deref());
    element_opt(out, indent + 1, "port", loc.port.as_deref());
    element_opt(out, indent + 1, "path", loc.path.as_deref());
    element_opt(out, indent + 1, "query", loc.query.as_deref());
    pad(out, indent);
    out.push_str(&format!("</{tag}>\n"));
}

fn tool_entry(name: &str, inner: &str) -> String {
    let mut out = String::new();
    tool_entry_into(&mut out, name, inner, 3);
    out
}

fn tool_entry_into(out: &mut String, name: &str, inner: &str, indent: usize) {
    pad(out, indent);
    out.push_str("<entry>\n");
    element(out, indent + 1, "string", name);
    out.push_str(inner);
    pad(out, indent);
    out.push_str("</entry>\n");
}

fn empty_config() -> String {
    let mut out = header();
    out.push_str("<configuration>\n");
    out.push_str("  <toolConfiguration>\n");
    out.push_str("    <configs>\n");
    out.push_str("    </configs>\n");
    out.push_str("  </toolConfiguration>\n");
    out.push_str("</configuration>\n");
    out
}

fn ensure_tool_config(xml: String) -> String {
    if xml.contains("<configs>") || xml.contains("<configs ") {
        return xml;
    }
    if let Some(pos) = xml.find("</toolConfiguration>") {
        let mut out = xml;
        out.insert_str(pos, "    <configs>\n    </configs>\n");
        return out;
    }
    let insertion =
        "  <toolConfiguration>\n    <configs>\n    </configs>\n  </toolConfiguration>\n";
    for close in ["</configuration>", "</charles-export>"] {
        if let Some(pos) = xml.find(close) {
            let mut out = xml;
            out.insert_str(pos, insertion);
            return out;
        }
    }
    let mut out = xml;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(insertion);
    out
}

fn upsert_tool_entry(xml: &str, tool_name: &str, new_entry: &str) -> String {
    let marker = format!("<string>{}</string>", escape(tool_name));
    if let Some(marker_pos) = xml.find(&marker)
        && let Some(start) = xml[..marker_pos].rfind("<entry>")
        && let Some(end_rel) = xml[marker_pos..].find("</entry>")
    {
        let end = marker_pos + end_rel + "</entry>".len();
        let mut out = xml.to_string();
        out.replace_range(start..end, new_entry.trim_end());
        return out;
    }

    if let Some(pos) = xml.find("</configs>") {
        let mut out = xml.to_string();
        out.insert_str(pos, new_entry);
        return out;
    }

    let mut out = ensure_tool_config(xml.to_string());
    if let Some(pos) = out.find("</configs>") {
        out.insert_str(pos, new_entry);
    }
    out
}

fn header() -> String {
    "<?xml version='1.0' encoding='UTF-8' ?>\n<?charles serialisation-version='2.0' ?>\n"
        .to_string()
}

fn element_opt(out: &mut String, indent: usize, tag: &str, value: Option<&str>) {
    if let Some(value) = value
        && !value.is_empty()
    {
        element(out, indent, tag, value);
    }
}

fn element(out: &mut String, indent: usize, tag: &str, value: &str) {
    pad(out, indent);
    out.push_str(&format!("<{tag}>{}</{tag}>\n", escape(value)));
}

fn element_bool(out: &mut String, indent: usize, tag: &str, value: bool) {
    element(out, indent, tag, if value { "true" } else { "false" });
}

fn element_num(out: &mut String, indent: usize, tag: &str, value: u8) {
    element(out, indent, tag, &value.to_string());
}

fn pad(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(host: &str) -> LocationReq {
        LocationReq {
            protocol: Some("https".into()),
            host: Some(host.into()),
            port: Some("443".into()),
            path: Some("/api".into()),
            query: None,
        }
    }

    fn base_req() -> WriteRulesReq {
        WriteRulesReq {
            path: "/tmp/rules.xml".into(),
            enable_tools: false,
            save_to_charles_config: false,
            config_path: None,
            confirm: false,
            map_local: vec![],
            map_remote: vec![],
            rewrite_sets: vec![],
        }
    }

    #[test]
    fn builds_map_local_settings_and_escapes_xml() {
        let req = WriteRulesReq {
            map_local: vec![MapLocalRuleReq {
                from: loc("api.example.com"),
                local_path: "/tmp/a&b.json".into(),
                enabled: true,
                case_sensitive: true,
            }],
            ..base_req()
        };
        let xml = build_rule_file(&req).unwrap();
        assert!(xml.contains("<charles-export>"));
        assert!(xml.contains("<mapLocal>"));
        assert!(xml.contains("<dest>/tmp/a&amp;b.json</dest>"));
        assert!(xml.contains("<toolEnabled>true</toolEnabled>"));
    }

    #[test]
    fn builds_rewrite_settings_with_body_rule() {
        let req = WriteRulesReq {
            rewrite_sets: vec![RewriteSetReq {
                name: "replace body".into(),
                active: true,
                locations: vec![loc("api.example.com")],
                rules: vec![RewriteRuleReq {
                    kind: Some(RewriteRuleKind::Body),
                    rule_type: None,
                    active: true,
                    match_header: None,
                    match_value: Some("old".into()),
                    match_header_regex: false,
                    match_value_regex: false,
                    match_request: Some(false),
                    match_response: Some(true),
                    new_header: None,
                    new_value: Some("new".into()),
                    new_header_regex: false,
                    new_value_regex: false,
                    match_whole_value: false,
                    case_sensitive: false,
                    replace_type: None,
                }],
            }],
            ..base_req()
        };
        let xml = build_rule_file(&req).unwrap();
        assert!(xml.contains("<rewrite>"));
        assert!(xml.contains("<ruleType>7</ruleType>"));
        assert!(xml.contains("<matchResponse>true</matchResponse>"));
    }

    #[test]
    fn rejects_relative_paths() {
        assert!(validate_output_path("rules.xml").is_err());
        let req = WriteRulesReq {
            map_local: vec![MapLocalRuleReq {
                from: loc("api.example.com"),
                local_path: "relative.json".into(),
                enabled: true,
                case_sensitive: true,
            }],
            ..base_req()
        };
        assert!(build_rule_file(&req).is_err());
    }

    #[test]
    fn merge_replaces_only_requested_tool_entries() {
        let existing = "<?xml version='1.0' encoding='UTF-8' ?>\n<configuration>\n  <proxyConfiguration><port>8888</port></proxyConfiguration>\n  <toolConfiguration>\n    <configs>\n      <entry>\n        <string>Map Local</string>\n        <mapLocal><toolEnabled>false</toolEnabled><mappings /></mapLocal>\n      </entry>\n      <entry>\n        <string>Rewrite</string>\n        <rewrite><toolEnabled>false</toolEnabled><sets /></rewrite>\n      </entry>\n    </configs>\n  </toolConfiguration>\n</configuration>\n";
        let req = WriteRulesReq {
            save_to_charles_config: true,
            confirm: true,
            map_local: vec![MapLocalRuleReq {
                from: loc("api.example.com"),
                local_path: "/tmp/mock.json".into(),
                enabled: true,
                case_sensitive: true,
            }],
            map_remote: vec![MapRemoteRuleReq {
                from: loc("prod.example.com"),
                to: loc("stage.example.com"),
                enabled: true,
                preserve_host_header: false,
            }],
            ..base_req()
        };
        let merged = merge_into_charles_config(existing, &req);
        assert!(merged.contains("<proxyConfiguration><port>8888</port></proxyConfiguration>"));
        assert!(merged.contains("<string>Map Local</string>"));
        assert!(merged.contains("<dest>/tmp/mock.json</dest>"));
        assert!(merged.contains("<string>Map Remote</string>"));
        assert!(merged.contains("<host>stage.example.com</host>"));
        assert!(merged.contains("<string>Rewrite</string>"));
        assert!(merged.contains("<rewrite><toolEnabled>false</toolEnabled><sets /></rewrite>"));
    }

    #[test]
    fn merge_creates_config_when_missing() {
        let req = WriteRulesReq {
            save_to_charles_config: true,
            confirm: true,
            map_remote: vec![MapRemoteRuleReq {
                from: loc("prod.example.com"),
                to: loc("stage.example.com"),
                enabled: true,
                preserve_host_header: true,
            }],
            ..base_req()
        };
        let merged = merge_into_charles_config("", &req);
        assert!(merged.contains("<configuration>"));
        assert!(merged.contains("<string>Map Remote</string>"));
        assert!(merged.contains("<preserveHostHeader>true</preserveHostHeader>"));
    }
}
