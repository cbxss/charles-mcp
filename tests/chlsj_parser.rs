use charles_mcp::session::{Body, Transaction, body, chlsj};

fn load() -> Vec<Transaction> {
    chlsj::parse(include_bytes!("fixtures/sample.chlsj")).expect("parse chlsj")
}

#[test]
fn parses_all_entries() {
    assert_eq!(load().len(), 5);
}

#[test]
fn first_entry_fields() {
    let txns = load();
    let t = &txns[0];
    assert_eq!(t.method, "GET");
    assert_eq!(t.scheme, "https");
    assert_eq!(t.host, "api.example.com");
    assert_eq!(t.path, "/v1/users?page=1");
    assert_eq!(t.status, Some(200));
    assert_eq!(t.mime.as_deref(), Some("application/json"));
    assert_eq!(t.tls_version.as_deref(), Some("TLSv1.3"));
    assert_eq!(t.remote_addr.as_deref(), Some("203.0.113.10"));
    assert_eq!(t.client_addr.as_deref(), Some("127.0.0.1"));
    assert_eq!(t.duration_ms, Some(53.0));
    assert!(t.started.is_some());
}

#[test]
fn url_reconstructed_omits_default_port() {
    let txns = load();
    assert_eq!(txns[0].url, "https://api.example.com/v1/users?page=1");
    assert_eq!(txns[4].url, "http://status.other.org/health");
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
fn gzip_response_body_decodes() {
    let txns = load();
    let resp = txns[1].response.as_ref().unwrap();
    match body::decode(&resp.raw, 8192) {
        Body::Text { text, .. } => assert!(text.contains("hello gzipped world")),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn request_body_text() {
    let txns = load();
    match body::decode(&txns[1].request.raw, 8192) {
        Body::Text { text, .. } => assert!(text.contains("ada")),
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn binary_body_detected() {
    let txns = load();
    let resp = txns[2].response.as_ref().unwrap();
    assert!(matches!(body::decode(&resp.raw, 8192), Body::Binary { .. }));
}

#[test]
fn failed_transaction() {
    let txns = load();
    let t = &txns[3];
    assert_eq!(t.status, None);
    assert!(t.error.is_some(), "expected a session error to be recorded");
    assert!(t.response.is_none());
}

#[test]
fn http_status_503() {
    let txns = load();
    assert_eq!(txns[4].status, Some(503));
    assert_eq!(txns[4].mime.as_deref(), Some("text/plain"));
}
