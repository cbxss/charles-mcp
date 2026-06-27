use charles_mcp::session::{Body, Transaction, body, har};

fn load() -> Vec<Transaction> {
    har::parse(include_bytes!("fixtures/sample.har")).expect("parse har")
}

#[test]
fn parses_all_entries() {
    assert_eq!(load().len(), 5);
}

#[test]
fn first_entry_fields() {
    let txns = load();
    let t = &txns[0];
    assert_eq!(t.index, 0);
    assert_eq!(t.method, "GET");
    assert_eq!(t.scheme, "https");
    assert_eq!(t.host, "api.example.com");
    assert_eq!(t.path, "/v1/users?page=1");
    assert_eq!(t.status, Some(200));
    assert_eq!(t.mime.as_deref(), Some("application/json"));
    assert_eq!(t.duration_ms, Some(53.0));
    assert!(t.started.is_some());
}

#[test]
fn duplicate_headers_preserved() {
    let txns = load();
    let resp = txns[0].response.as_ref().unwrap();
    let cookies = resp
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .count();
    assert_eq!(cookies, 2);
}

#[test]
fn plain_text_body() {
    let txns = load();
    let resp = txns[0].response.as_ref().unwrap();
    match body::decode(&resp.raw, 8192) {
        Body::Text { text, .. } => assert!(text.contains("Ada")),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn gzip_body_decodes() {
    let txns = load();
    let resp = txns[1].response.as_ref().unwrap();
    match body::decode(&resp.raw, 8192) {
        Body::Text { text, .. } => assert!(text.contains("hello gzipped world")),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn request_post_body() {
    let txns = load();
    match body::decode(&txns[1].request.raw, 8192) {
        Body::Text { text, .. } => assert!(text.contains("ada")),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn base64_binary_body_detected() {
    let txns = load();
    let resp = txns[2].response.as_ref().unwrap();
    assert!(matches!(body::decode(&resp.raw, 8192), Body::Binary { .. }));
    assert_eq!(txns[2].mime.as_deref(), Some("image/png"));
}

#[test]
fn missing_response_is_none() {
    let txns = load();
    let t = &txns[3];
    assert_eq!(t.status, None);
    assert!(t.response.is_none());
}

#[test]
fn multi_host_and_status() {
    let txns = load();
    assert_eq!(txns[4].host, "status.other.org");
    assert_eq!(txns[4].scheme, "http");
    assert_eq!(txns[4].status, Some(503));
}
