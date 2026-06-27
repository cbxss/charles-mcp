use charles_mcp::session::{Session, SessionSource, har};
use charles_mcp::tools::SearchField;
use charles_mcp::tools::inspect::{self, ListFilters, Matcher};

fn session() -> Session {
    Session {
        source: SessionSource::File("sample.har".into()),
        transactions: har::parse(include_bytes!("fixtures/sample.har")).unwrap(),
    }
}

fn base<'a>() -> ListFilters<'a> {
    ListFilters {
        host: None,
        method: None,
        status: None,
        path_regex: None,
        mime: None,
        limit: 50,
    }
}

#[test]
fn filter_by_host() {
    let s = session();
    let r = inspect::list(
        &s,
        &ListFilters {
            host: Some("api.example.com"),
            ..base()
        },
    );
    assert_eq!(r.total, 3);
}

#[test]
fn filter_by_method() {
    let s = session();
    let r = inspect::list(
        &s,
        &ListFilters {
            method: Some("post"),
            ..base()
        },
    );
    assert_eq!(r.total, 1);
    assert_eq!(r.rows[0].method, "POST");
}

#[test]
fn filter_by_status() {
    let s = session();
    let r = inspect::list(
        &s,
        &ListFilters {
            status: Some(200),
            ..base()
        },
    );
    assert_eq!(r.total, 3);
}

#[test]
fn filter_by_mime() {
    let s = session();
    let r = inspect::list(
        &s,
        &ListFilters {
            mime: Some("json"),
            ..base()
        },
    );
    assert_eq!(r.total, 2);
}

#[test]
fn filter_by_path_regex() {
    let s = session();
    let r = inspect::list(
        &s,
        &ListFilters {
            path_regex: Some(regex::Regex::new(r"^/v1/").unwrap()),
            ..base()
        },
    );
    assert_eq!(r.total, 3);
}

#[test]
fn limit_truncates_but_reports_total() {
    let s = session();
    let r = inspect::list(&s, &ListFilters { limit: 2, ..base() });
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.total, 5);
}

#[test]
fn search_url_substring() {
    let s = session();
    let m = Matcher::build("logo.png", false).unwrap();
    let hits = inspect::search(&s, &m, &[SearchField::Url], 50);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].index, 2);
}

#[test]
fn search_headers() {
    let s = session();
    let m = Matcher::build("gzip", false).unwrap();
    let hits = inspect::search(&s, &m, &[SearchField::Headers], 50);
    assert!(hits.iter().any(|h| h.index == 1 && h.field == "headers"));
}

#[test]
fn search_body_decodes_gzip() {
    let s = session();
    let m = Matcher::build("gzipped world", false).unwrap();
    let hits = inspect::search(&s, &m, &[SearchField::Body], 50);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].index, 1);
    assert_eq!(hits[0].field, "body");
}

#[test]
fn search_regex_across_all_fields() {
    let s = session();
    let m = Matcher::build(r"/v1/(login|users)", true).unwrap();
    let hits = inspect::search(&s, &m, &[], 50);
    let idxs: Vec<_> = hits.iter().map(|h| h.index).collect();
    assert!(idxs.contains(&0));
    assert!(idxs.contains(&1));
}

#[test]
fn stats_aggregate() {
    let s = session();
    let st = inspect::stats(&s);
    assert_eq!(st.total, 5);
    assert_eq!(st.errors, 1);
    assert_eq!(st.total_response_bytes, 36 + 39 + 20 + 11);
    assert_eq!(st.by_host[0], ("api.example.com".to_string(), 3));
}
