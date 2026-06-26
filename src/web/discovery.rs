//! Runtime discovery of the *undocumented* Charles Web Interface endpoints.
//!
//! Charles exposes session export/download/clear and an application-quit action
//! through `http://control.charles/`, but the exact paths are not part of the
//! public documentation and have shifted between versions. Rather than hardcode
//! guesses, we fetch the control page and parse its `<form>`/`<a>` elements to
//! learn the real action paths (and, for export, the available formats).
//!
//! This module is pure: [`discover_from_html`] takes the page HTML and returns
//! the classified endpoints, so it is fully unit-testable without a live Charles.

use scraper::{ElementRef, Html, Selector};

/// A single discovered control endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct EndpointSpec {
    /// HTTP method, uppercased. Defaults to `"GET"`.
    pub method: String,
    /// Path (plus query, if any) relative to the control host, with no leading
    /// scheme/host and no leading slash, e.g. `"session/export-session"`.
    pub path: String,
    /// Name of the `<select>` controlling the export format, if present.
    pub format_field: Option<String>,
    /// Lower-cased option values of that select (e.g. `chlsj`, `har`, `xml`).
    pub formats: Vec<String>,
}

impl EndpointSpec {
    fn new(method: &str, path: String) -> Self {
        EndpointSpec {
            method: method.to_uppercase(),
            path,
            format_field: None,
            formats: Vec::new(),
        }
    }
}

/// The control endpoints we care about, each optional (absent → `None`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveredEndpoints {
    /// Export the current session in a chosen format.
    pub export: Option<EndpointSpec>,
    /// Download the native `.chls` session.
    pub download_chls: Option<EndpointSpec>,
    /// Clear the current session.
    pub clear: Option<EndpointSpec>,
    /// Quit Charles.
    pub quit: Option<EndpointSpec>,
}

/// Parse the HTML of `http://control.charles/` and extract the session
/// export/download/clear/quit endpoints. Unknown or missing elements simply
/// leave the corresponding field as `None`.
pub fn discover_from_html(html: &str) -> DiscoveredEndpoints {
    let mut endpoints = DiscoveredEndpoints::default();
    if html.trim().is_empty() {
        return endpoints;
    }

    let doc = Html::parse_document(html);
    let form_sel = Selector::parse("form").unwrap();
    let a_sel = Selector::parse("a").unwrap();
    let select_sel = Selector::parse("select").unwrap();
    let option_sel = Selector::parse("option").unwrap();

    // Forms first — they carry method + the format <select>.
    for form in doc.select(&form_sel) {
        let action = form.value().attr("action").unwrap_or("");
        let path = normalize_path(action);
        let method = form.value().attr("method").unwrap_or("get");
        let label = element_text(&form);

        let mut spec = EndpointSpec::new(method, path.clone());
        if let Some(sel) = form.select(&select_sel).next() {
            spec.format_field = sel.value().attr("name").map(str::to_string);
            for opt in sel.select(&option_sel) {
                let v = opt
                    .value()
                    .attr("value")
                    .map(str::to_string)
                    .unwrap_or_else(|| opt.text().collect::<String>());
                let v = v.trim().to_lowercase();
                if !v.is_empty() {
                    spec.formats.push(v);
                }
            }
        }
        let has_formats = !spec.formats.is_empty();
        classify(&mut endpoints, &path, &label, spec, has_formats);
    }

    // Then anchor links (always GET).
    for a in doc.select(&a_sel) {
        let href = a.value().attr("href").unwrap_or("");
        if href.is_empty() || href.starts_with('#') {
            continue;
        }
        let path = normalize_path(href);
        let label = a.text().collect::<String>();
        let spec = EndpointSpec::new("GET", path.clone());
        classify(&mut endpoints, &path, &label, spec, false);
    }

    endpoints
}

/// Route a candidate endpoint into the right slot based on path + label keywords.
fn classify(
    ep: &mut DiscoveredEndpoints,
    path: &str,
    label: &str,
    spec: EndpointSpec,
    has_format_select: bool,
) {
    let hay = format!("{path} {label}").to_lowercase();
    let has = |k: &str| hay.contains(k);

    // A format <select> uniquely marks the export endpoint — check it FIRST so
    // an export form whose option labels mention ".chlsj"/"native" isn't
    // misread as the download endpoint. (`chls` is intentionally NOT a download
    // keyword for that reason.)
    if has_format_select && (has("export") || has("session")) {
        set_export(&mut ep.export, spec);
        return;
    }

    // Order matters: `clear`/`quit`/`download` before the export fallback so a
    // "clear session" control is never misread as the export endpoint.
    if has("clear") {
        set_first(&mut ep.clear, spec);
    } else if has("quit") || has("shutdown") || has("exit") {
        set_first(&mut ep.quit, spec);
    } else if has("download") || has("native") {
        set_first(&mut ep.download_chls, spec);
    } else if has("export") {
        set_export(&mut ep.export, spec);
    }
}

fn set_first(slot: &mut Option<EndpointSpec>, spec: EndpointSpec) {
    if slot.is_none() {
        *slot = Some(spec);
    }
}

/// Prefer an export spec that actually carries a format list.
fn set_export(slot: &mut Option<EndpointSpec>, spec: EndpointSpec) {
    match slot {
        None => *slot = Some(spec),
        Some(existing) if existing.formats.is_empty() && !spec.formats.is_empty() => {
            *slot = Some(spec)
        }
        _ => {}
    }
}

/// Strip any `control.charles` host prefix and the leading slash from an href/action.
fn normalize_path(raw: &str) -> String {
    let mut p = raw.trim();
    for prefix in [
        "http://control.charles",
        "https://control.charles",
        "//control.charles",
    ] {
        if let Some(rest) = p.strip_prefix(prefix) {
            p = rest;
            break;
        }
    }
    p.trim_start_matches('/').to_string()
}

/// Visible text of an element, plus any submit/button `value` labels.
fn element_text(el: &ElementRef) -> String {
    let mut s = el.text().collect::<Vec<_>>().join(" ");
    let input_sel = Selector::parse("input, button").unwrap();
    for inp in el.select(&input_sel) {
        if let Some(v) = inp.value().attr("value") {
            s.push(' ');
            s.push_str(v);
        }
    }
    s
}
