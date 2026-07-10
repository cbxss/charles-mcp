use std::collections::HashMap;

use regex::Regex;

use crate::error::CharlesError;
use crate::session::{Body, HttpMessage, Session, Transaction, TxnSummary, body};
use crate::tools::SearchField;

pub struct ListResult {
    pub rows: Vec<TxnSummary>,
    pub total: usize,
}

#[derive(Debug, Clone)]
pub struct ListFilters<'a> {
    pub host: Option<&'a str>,
    pub method: Option<&'a str>,
    pub status: Option<u16>,
    pub path_regex: Option<Regex>,
    pub mime: Option<&'a str>,
    pub limit: usize,
}

pub fn list(session: &Session, f: &ListFilters) -> ListResult {
    let matches: Vec<&Transaction> = session
        .transactions
        .iter()
        .filter(|t| matches_filters(t, f))
        .collect();
    let total = matches.len();
    let rows = matches
        .into_iter()
        .take(f.limit)
        .map(Transaction::summary)
        .collect();
    ListResult { rows, total }
}

fn matches_filters(t: &Transaction, f: &ListFilters) -> bool {
    if let Some(h) = f.host
        && !t.host.to_lowercase().contains(&h.to_lowercase())
    {
        return false;
    }
    if let Some(m) = f.method
        && !t.method.eq_ignore_ascii_case(m)
    {
        return false;
    }
    if let Some(s) = f.status
        && t.status != Some(s)
    {
        return false;
    }
    if let Some(re) = &f.path_regex
        && !re.is_match(&t.path)
    {
        return false;
    }
    if let Some(m) = f.mime {
        let m = m.to_lowercase();
        if !t
            .mime
            .as_deref()
            .map(|x| x.to_lowercase().contains(&m))
            .unwrap_or(false)
        {
            return false;
        }
    }
    true
}

pub enum Matcher {
    Regex(Regex),
    Substr(String),
}

impl Matcher {
    pub fn build(query: &str, regex: bool) -> Result<Matcher, CharlesError> {
        if regex {
            Ok(Matcher::Regex(Regex::new(query).map_err(|e| {
                CharlesError::InvalidArg(format!("bad regex: {e}"))
            })?))
        } else {
            Ok(Matcher::Substr(query.to_ascii_lowercase()))
        }
    }

    fn find(&self, hay: &str) -> Option<usize> {
        match self {
            Matcher::Regex(re) => re.find(hay).map(|m| m.start()),
            Matcher::Substr(q) => hay.to_ascii_lowercase().find(q),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub index: usize,
    pub field: &'static str,
    pub snippet: String,
}

pub fn search(
    session: &Session,
    matcher: &Matcher,
    fields: &[SearchField],
    limit: usize,
) -> Vec<SearchHit> {
    let want = |f: SearchField| fields.is_empty() || fields.contains(&f);
    let mut hits = Vec::new();
    for t in &session.transactions {
        if hits.len() >= limit {
            break;
        }
        if want(SearchField::Url)
            && let Some(pos) = matcher.find(&t.url)
        {
            hits.push(SearchHit {
                index: t.index,
                field: "url",
                snippet: snippet(&t.url, pos),
            });
            continue;
        }
        if want(SearchField::Headers)
            && let Some(s) = search_headers(matcher, &t.request, &t.response)
        {
            hits.push(SearchHit {
                index: t.index,
                field: "headers",
                snippet: s,
            });
            continue;
        }
        if want(SearchField::Body)
            && let Some(s) = search_body(matcher, t)
        {
            hits.push(SearchHit {
                index: t.index,
                field: "body",
                snippet: s,
            });
            continue;
        }
        if want(SearchField::Body)
            && let Some(s) = search_ws(matcher, t)
        {
            hits.push(SearchHit {
                index: t.index,
                field: "ws",
                snippet: s,
            });
            continue;
        }
    }
    hits
}

fn search_headers(
    matcher: &Matcher,
    req: &HttpMessage,
    resp: &Option<HttpMessage>,
) -> Option<String> {
    let check = |msg: &HttpMessage| -> Option<String> {
        for (k, v) in &msg.headers {
            let line = format!("{k}: {v}");
            if let Some(pos) = matcher.find(&line) {
                return Some(snippet(&line, pos));
            }
        }
        None
    };
    check(req).or_else(|| resp.as_ref().and_then(check))
}

fn search_body(matcher: &Matcher, t: &Transaction) -> Option<String> {
    const SEARCH_CAP: usize = 1 << 20;
    let texts = [Some(&t.request), t.response.as_ref()];
    for msg in texts.into_iter().flatten() {
        let hay = match body::decode(&msg.raw, SEARCH_CAP) {
            Body::Text { text, .. } => Some(text),
            Body::Protobuf { tree, .. } => Some(tree),
            _ => None,
        };
        if let Some(hay) = hay
            && let Some(pos) = matcher.find(&hay)
        {
            return Some(snippet(&hay, pos));
        }
    }
    None
}

fn search_ws(matcher: &Matcher, t: &Transaction) -> Option<String> {
    const SEARCH_CAP: usize = 1 << 20;
    let frames = t.websocket.as_ref()?;
    for m in frames {
        let hay = body::ws_frame_text(&m.payload, SEARCH_CAP);
        if hay.is_empty() {
            continue;
        }
        if let Some(pos) = matcher.find(&hay) {
            return Some(snippet(&hay, pos));
        }
    }
    None
}

fn snippet(hay: &str, pos: usize) -> String {
    const CTX: usize = 40;
    let start = pos.saturating_sub(CTX);
    let end = (pos + CTX).min(hay.len());
    let start = floor_boundary(hay, start);
    let end = ceil_boundary(hay, end);
    let mut s = hay[start..end].replace(['\n', '\r', '\t'], " ");
    if start > 0 {
        s.insert(0, '…');
    }
    if end < hay.len() {
        s.push('…');
    }
    s
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub total: usize,
    pub errors: usize,
    pub total_response_bytes: u64,
    pub by_host: Vec<(String, usize)>,
    pub by_status: Vec<(String, usize)>,
    pub by_mime: Vec<(String, usize)>,
    pub slowest: Vec<(usize, String, f64)>,
}

pub fn stats(session: &Session) -> Stats {
    let txns = &session.transactions;
    let mut by_host: HashMap<String, usize> = HashMap::new();
    let mut by_status: HashMap<String, usize> = HashMap::new();
    let mut by_mime: HashMap<String, usize> = HashMap::new();
    let mut total_response_bytes = 0u64;
    let mut errors = 0usize;

    for t in txns {
        *by_host.entry(t.host.clone()).or_default() += 1;
        let status_key = t
            .status
            .map(|s| s.to_string())
            .unwrap_or_else(|| "(none)".to_string());
        *by_status.entry(status_key).or_default() += 1;
        *by_mime
            .entry(t.mime.clone().unwrap_or_else(|| "(none)".to_string()))
            .or_default() += 1;
        total_response_bytes += t.response_size.unwrap_or(0);
        if t.error.is_some() || t.status.map(|s| s >= 400).unwrap_or(false) {
            errors += 1;
        }
    }

    let mut slowest: Vec<(usize, String, f64)> = txns
        .iter()
        .filter_map(|t| t.duration_ms.map(|d| (t.index, t.url.clone(), d)))
        .collect();
    slowest.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    slowest.truncate(5);

    Stats {
        total: txns.len(),
        errors,
        total_response_bytes,
        by_host: sorted_desc(by_host),
        by_status: sorted_desc(by_status),
        by_mime: sorted_desc(by_mime),
        slowest,
    }
}

fn sorted_desc(map: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}
