use scraper::{ElementRef, Html, Selector};

#[derive(Debug, Clone, PartialEq)]
pub struct EndpointSpec {
    pub method: String,
    pub path: String,
    pub format_field: Option<String>,
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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiscoveredEndpoints {
    pub export: Option<EndpointSpec>,
    pub download_chls: Option<EndpointSpec>,
    pub clear: Option<EndpointSpec>,
    pub quit: Option<EndpointSpec>,
}

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

fn classify(
    ep: &mut DiscoveredEndpoints,
    path: &str,
    label: &str,
    spec: EndpointSpec,
    has_format_select: bool,
) {
    let hay = format!("{path} {label}").to_lowercase();
    let has = |k: &str| hay.contains(k);

    if has_format_select && (has("export") || has("session")) {
        set_export(&mut ep.export, spec);
        return;
    }

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

fn set_export(slot: &mut Option<EndpointSpec>, spec: EndpointSpec) {
    match slot {
        None => *slot = Some(spec),
        Some(existing) if existing.formats.is_empty() && !spec.formats.is_empty() => {
            *slot = Some(spec)
        }
        _ => {}
    }
}

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
