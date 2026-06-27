//! Integration tests for the SQLite traffic store: ingest a parsed `Session`
//! once, then exercise the public query surface (list/get/search/stats) plus the
//! content-addressed body dedup that keeps repeated payloads stored once.

use charles_mcp::session::{HttpMessage, RawBody, Session, SessionSource, Transaction};
use charles_mcp::store::{StoreFilters, TrafficStore};

/// Build a minimal but realistic transaction. The request body is left
/// uncaptured (only a Content-Type), so only the captured response body is
/// content-addressed into the `bodies` table.
fn txn(
    seq: usize,
    method: &str,
    host: &str,
    path: &str,
    status: u16,
    mime: &str,
    body: &[u8],
) -> Transaction {
    Transaction {
        index: seq,
        scheme: "https".into(),
        host: host.into(),
        method: method.into(),
        path: path.into(),
        url: format!("https://{host}{path}"),
        status: Some(status),
        mime: Some(mime.into()),
        response_size: Some(body.len() as u64),
        request: HttpMessage {
            headers: vec![("Accept".into(), "application/json".into())],
            raw: RawBody {
                content_type: Some(mime.into()),
                ..Default::default()
            },
        },
        response: Some(HttpMessage {
            headers: vec![("Content-Type".into(), mime.into())],
            raw: RawBody {
                bytes: body.to_vec(),
                content_type: Some(mime.into()),
                captured: true,
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

/// Four transactions spanning the classifier's range: a static PNG, an HTML
/// document, a JSON login POST (priority 100), and a JSON 500 error (priority
/// 105). Their natural `seq` order is the reverse of their priority order, so
/// `list` ordering is exercised meaningfully.
fn sample_session() -> Session {
    Session {
        source: SessionSource::Live,
        transactions: vec![
            // seq 0 — static_asset (priority 5)
            txn(
                0,
                "GET",
                "cdn.example.com",
                "/assets/logo.png",
                200,
                "image/png",
                b"\x89PNG\r\n\x1a\nlogo-bytes",
            ),
            // seq 1 — document (priority 40)
            txn(
                1,
                "GET",
                "example.com",
                "/index.html",
                200,
                "text/html",
                b"<html><body>home</body></html>",
            ),
            // seq 2 — api_candidate POST (20 + 40 json + 25 api hint + 15 mutating = 100)
            txn(
                2,
                "POST",
                "api.example.com",
                "/api/login",
                200,
                "application/json",
                br#"{"token":"abc","user":"sam"}"#,
            ),
            // seq 3 — api_candidate GET 500 (20 + 40 json + 25 api hint + 20 error = 105)
            txn(
                3,
                "GET",
                "api.example.com",
                "/api/orders",
                500,
                "application/json",
                br#"{"error":"boom"}"#,
            ),
        ],
    }
}

#[test]
fn round_trip_list_get() {
    let store = TrafficStore::open(None).unwrap();
    let session = sample_session();
    let cap = store
        .ingest("live", "live", None, None, &session, 65536)
        .unwrap();
    assert_eq!(cap.entry_count, 4);

    let (rows, total) = store
        .list(
            "live",
            &StoreFilters {
                limit: 50,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(total, 4);
    assert_eq!(rows.len(), 4);

    // Ordered by priority DESC: each row's priority is >= the next's.
    for w in rows.windows(2) {
        assert!(
            w[0].priority >= w[1].priority,
            "rows not ordered by priority DESC"
        );
    }
    // The api_candidate POST (seq 2) must sort before the static_asset PNG (seq 0).
    let pos = |seq: usize| rows.iter().position(|r| r.seq == seq).unwrap();
    assert!(pos(2) < pos(0), "api POST should outrank the static asset");
    assert_eq!(rows[pos(2)].resource_class, "api_candidate");
    assert_eq!(rows[pos(0)].resource_class, "static_asset");

    // Reconstruct the POST and confirm the body + headers round-tripped.
    let got = store.get("live", 2).unwrap().expect("seq 2 exists");
    assert_eq!(got.method, "POST");
    let resp = got.response.expect("response present");
    assert_eq!(resp.raw.bytes, br#"{"token":"abc","user":"sam"}"#.to_vec());
    assert_eq!(got.request.header("Accept"), Some("application/json"));
}

#[test]
fn fts_finds_decoded_body() {
    let store = TrafficStore::open(None).unwrap();
    let session = sample_session();
    store
        .ingest("live", "live", None, None, &session, 65536)
        .unwrap();

    // "token" appears only in the login response body (indexed decoded).
    let hits = store.search_fts("live", "token", 10).unwrap();
    assert!(
        !hits.is_empty(),
        "expected at least one FTS hit for 'token'"
    );
    assert!(
        hits.iter().any(|(seq, _)| *seq == 2),
        "the login POST (seq 2) should match"
    );
}

#[test]
fn content_addressed_dedup() {
    // Persist to a real file so a second rusqlite connection can count body rows.
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("store.db");

    // Two transactions share an identical response body; a third differs. So two
    // distinct body contents are ingested across three entries.
    let shared = br#"{"payload":"identical bytes"}"#;
    let distinct = br#"{"payload":"different bytes"}"#;
    let session = Session {
        source: SessionSource::Live,
        transactions: vec![
            txn(
                0,
                "POST",
                "api.example.com",
                "/api/a",
                200,
                "application/json",
                shared,
            ),
            txn(
                1,
                "POST",
                "api.example.com",
                "/api/b",
                200,
                "application/json",
                shared,
            ),
            txn(
                2,
                "POST",
                "api.example.com",
                "/api/c",
                200,
                "application/json",
                distinct,
            ),
        ],
    };

    {
        let store = TrafficStore::open(Some(&db_path)).unwrap();
        let cap = store
            .ingest("live", "live", None, None, &session, 65536)
            .unwrap();
        assert_eq!(cap.entry_count, 3);
    }

    // Independent connection to the same DB: the bodies table holds one row per
    // distinct content (2), not one per entry (3) — request bodies are uncaptured.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let body_count: i64 = conn
        .query_row("SELECT count(*) FROM bodies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        body_count, 2,
        "identical response bodies should dedup to one row"
    );
}

#[test]
fn filters_and_stats() {
    let store = TrafficStore::open(None).unwrap();
    let session = sample_session();
    store
        .ingest("live", "live", None, None, &session, 65536)
        .unwrap();

    // resource_class filter returns only the two api_candidate entries.
    let (rows, total) = store
        .list(
            "live",
            &StoreFilters {
                resource_class: Some("api_candidate".into()),
                limit: 50,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(total, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.resource_class == "api_candidate"));

    let stats = store.stats("live").unwrap();
    assert_eq!(stats.total, 4);
    assert_eq!(stats.errors, 1, "only the 500 response is an error");
}
