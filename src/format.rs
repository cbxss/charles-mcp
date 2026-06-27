//! Compact, context-frugal rendering of session summaries and details.

use crate::replay::ReplayResult;
use crate::session::{Body, Transaction, TxnSummary};
use crate::store::EntryRow;
use crate::tools::inspect::{SearchHit, Stats};

/// Render a list of summaries as an aligned, compact table.
pub fn summary_table(rows: &[TxnSummary]) -> String {
    if rows.is_empty() {
        return "requests: 0 total (no rows matched the filters)".to_string();
    }
    // Column widths (path is truncated to keep rows scannable).
    const PATH_MAX: usize = 48;
    const HOST_MAX: usize = 28;

    let host_w = rows
        .iter()
        .map(|r| r.host.len().min(HOST_MAX))
        .max()
        .unwrap_or(4)
        .max(4);

    let mut out = String::new();
    out.push_str(&format!(
        "{:>4}  {:<6} {:>3}  {:<hw$}  {:<pw$}  {:<18} {:>8}  {:>6}\n",
        "#",
        "METHOD",
        "ST",
        "HOST",
        "PATH",
        "MIME",
        "SIZE",
        "MS",
        hw = host_w,
        pw = PATH_MAX,
    ));
    for r in rows {
        out.push_str(&format!(
            "{:>4}  {:<6} {:>3}  {:<hw$}  {:<pw$}  {:<18} {:>8}  {:>6}\n",
            r.index,
            truncate(&r.method, 6),
            r.status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into()),
            truncate(&r.host, host_w),
            truncate(&r.path, PATH_MAX),
            truncate(r.mime.as_deref().unwrap_or("-"), 18),
            r.response_size
                .map(human_size)
                .unwrap_or_else(|| "-".into()),
            r.duration_ms
                .map(|d| format!("{:.0}", d))
                .unwrap_or_else(|| "-".into()),
            hw = host_w,
            pw = PATH_MAX,
        ));
    }
    out
}

/// Render store entry rows as a table that also carries the resource-class tag
/// (so API/JSON/error traffic is distinguishable from static-asset noise) — the
/// rows arrive pre-sorted by priority.
pub fn entry_table(rows: &[EntryRow]) -> String {
    if rows.is_empty() {
        return "requests: 0 total (no rows matched the filters)".to_string();
    }
    const PATH_MAX: usize = 40;
    const HOST_MAX: usize = 26;
    const CLASS_MAX: usize = 14;

    let host_w = rows
        .iter()
        .map(|r| r.host.len().min(HOST_MAX))
        .max()
        .unwrap_or(4)
        .max(4);

    let mut out = String::new();
    out.push_str(&format!(
        "{:>4}  {:<6} {:>3}  {:<hw$}  {:<pw$}  {:<14} {:<cw$} {:>8}  {:>6}\n",
        "#",
        "METHOD",
        "ST",
        "HOST",
        "PATH",
        "MIME",
        "CLASS",
        "SIZE",
        "MS",
        hw = host_w,
        pw = PATH_MAX,
        cw = CLASS_MAX,
    ));
    for r in rows {
        out.push_str(&format!(
            "{:>4}  {:<6} {:>3}  {:<hw$}  {:<pw$}  {:<14} {:<cw$} {:>8}  {:>6}\n",
            r.seq,
            truncate(&r.method, 6),
            r.status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into()),
            truncate(&r.host, host_w),
            truncate(&r.path, PATH_MAX),
            truncate(r.mime.as_deref().unwrap_or("-"), 14),
            truncate(&r.resource_class, CLASS_MAX),
            r.response_size
                .map(human_size)
                .unwrap_or_else(|| "-".into()),
            r.duration_ms
                .map(|d| format!("{:.0}", d))
                .unwrap_or_else(|| "-".into()),
            hw = host_w,
            pw = PATH_MAX,
            cw = CLASS_MAX,
        ));
    }
    out
}

/// Render the full detail of one transaction, including a decoded body.
pub fn transaction_detail(t: &Transaction, req_body: &Body, resp_body: &Body) -> String {
    let mut out = String::new();
    out.push_str(&format!("#{} {} {}\n", t.index, t.method, t.url));
    if let Some(p) = &t.protocol {
        out.push_str(&format!("protocol: {p}\n"));
    }
    if let Some(s) = t.status {
        out.push_str(&format!(
            "status: {s}{}\n",
            t.status_text
                .as_deref()
                .map(|x| format!(" {x}"))
                .unwrap_or_default(),
        ));
    }
    if let Some(e) = &t.error {
        out.push_str(&format!("error: {e}\n"));
    }
    if t.tunnel {
        out.push_str(
            "⚠ HTTPS tunnel — NOT decrypted by Charles (SSL Proxying is off for this host). \
             Bodies below are ciphertext, not real content. Enable Proxy → SSL Proxying \
             Settings for this host to inspect it.\n",
        );
    }
    if let Some(d) = t.duration_ms {
        out.push_str(&format!("duration: {d:.0} ms\n"));
    }
    if t.client_addr.is_some() || t.remote_addr.is_some() || t.tls_version.is_some() {
        out.push_str(&format!(
            "client: {}  remote: {}  tls: {}\n",
            t.client_addr.as_deref().unwrap_or("-"),
            t.remote_addr.as_deref().unwrap_or("-"),
            t.tls_version.as_deref().unwrap_or("-"),
        ));
    }

    out.push_str("\n── request ──\n");
    render_headers(&mut out, &t.request.headers);
    out.push_str("\nbody:\n");
    render_body_or_tunnel(&mut out, req_body, t.tunnel);

    out.push_str("\n── response ──\n");
    match &t.response {
        Some(resp) => {
            render_headers(&mut out, &resp.headers);
            out.push_str("\nbody:\n");
            render_body_or_tunnel(&mut out, resp_body, t.tunnel);
        }
        None => out.push_str("(no response captured)\n"),
    }
    if let Some(frames) = &t.websocket {
        out.push_str(&format!(
            "\n── websocket ── {} frame(s); call get_websocket_messages for the decoded frames\n",
            frames.len()
        ));
    }
    out
}

fn render_body_or_tunnel(out: &mut String, body: &Body, tunnel: bool) {
    if tunnel {
        out.push_str("(encrypted — SSL Proxying not enabled for this host)\n");
    } else {
        render_body(out, body);
    }
}

fn render_headers(out: &mut String, headers: &[(String, String)]) {
    if headers.is_empty() {
        out.push_str("(no headers)\n");
        return;
    }
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\n"));
    }
}

/// Render a single decoded body to a string (used by get_websocket_messages).
pub fn render_body_str(body: &Body) -> String {
    let mut s = String::new();
    render_body(&mut s, body);
    s
}

fn render_body(out: &mut String, body: &Body) {
    match body {
        Body::Empty => out.push_str("(empty)\n"),
        Body::NotCaptured => out.push_str("(not captured)\n"),
        Body::Text {
            text,
            charset,
            truncated,
            original_len,
        } => {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
            if *truncated {
                out.push_str(&format!(
                    "… [truncated: {} of {} bytes, charset {}; raise max_body_bytes for more]\n",
                    text.len(),
                    original_len,
                    charset,
                ));
            }
        }
        Body::Binary {
            bytes_len,
            sample_hex,
            truncated,
        } => {
            out.push_str(&format!(
                "(binary, {} bytes) first bytes: {}{}\n",
                bytes_len,
                sample_hex,
                if *truncated { "…" } else { "" },
            ));
        }
        Body::Protobuf {
            tree,
            message_count,
            named,
            truncated,
            original_len,
        } => {
            let kind = if *named {
                "protobuf (named)"
            } else {
                "protobuf (schemaless; keys are field numbers, not names)"
            };
            if *message_count > 1 {
                out.push_str(&format!("({kind}, {message_count} gRPC messages)\n"));
            } else {
                out.push_str(&format!("({kind})\n"));
            }
            out.push_str(tree);
            if !tree.ends_with('\n') {
                out.push('\n');
            }
            if *truncated {
                out.push_str(&format!(
                    "… [truncated: {} of {} bytes; raise max_body_bytes for more]\n",
                    tree.len(),
                    original_len,
                ));
            }
        }
    }
}

/// Render a replay outcome: a one-line preview (method, URL, whether creds were
/// sent), the status vs. baseline, then response headers and the decoded body.
pub fn replay_report(r: &ReplayResult) -> String {
    let mut out = String::new();
    let creds = if r.auth_present {
        " (Authorization/Cookie sent)"
    } else {
        ""
    };
    let via = if r.via_proxy {
        " via Charles proxy"
    } else {
        " direct to origin"
    };
    out.push_str(&format!("replayed {} {}{creds}{via}\n", r.method, r.url));
    let baseline = r
        .baseline_status
        .map(|s| s.to_string())
        .unwrap_or_else(|| "-".into());
    let changed = r.baseline_status.map(|b| b != r.status).unwrap_or(true);
    out.push_str(&format!(
        "status: {} (baseline {}){}  ·  {} ms\n",
        r.status,
        baseline,
        if changed { " [changed]" } else { "" },
        r.elapsed_ms,
    ));
    out.push_str("\n── response headers ──\n");
    render_headers(&mut out, &r.response_headers);
    out.push_str("\nbody:\n");
    render_body(&mut out, &r.body);
    out
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes}{}", UNITS[0])
    } else {
        format!("{v:.1}{}", UNITS[i])
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Render search hits compactly: one line per hit with index, field, snippet.
pub fn search_results(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return "0 hits (no rows matched the query)".to_string();
    }
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!("#{:<4} [{}] {}\n", h.index, h.field, h.snippet));
    }
    out
}

/// Render aggregate session statistics.
pub fn stats_report(s: &Stats) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} transactions, {} error(s), {} total response bytes\n",
        s.total,
        s.errors,
        human_size(s.total_response_bytes),
    ));

    let section = |out: &mut String, title: &str, rows: &[(String, usize)], max: usize| {
        out.push_str(&format!("\n{title}:\n"));
        if rows.is_empty() {
            out.push_str("  (none)\n");
        }
        for (k, n) in rows.iter().take(max) {
            out.push_str(&format!("  {n:>5}  {k}\n"));
        }
    };
    section(&mut out, "by host", &s.by_host, 15);
    section(&mut out, "by status", &s.by_status, 15);
    section(&mut out, "by mime", &s.by_mime, 15);

    if !s.slowest.is_empty() {
        out.push_str("\nslowest:\n");
        for (idx, url, ms) in &s.slowest {
            out.push_str(&format!("  {ms:>7.0} ms  #{idx}  {}\n", truncate(url, 70)));
        }
    }
    out
}
