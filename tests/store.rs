use charles_mcp::session::{HttpMessage, RawBody, Session, SessionSource, Transaction};
use charles_mcp::store::{StoreFilters, TrafficStore};

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

fn sample_session() -> Session {
    Session {
        source: SessionSource::Live,
        transactions: vec![
            txn(
                0,
                "GET",
                "cdn.example.com",
                "/assets/logo.png",
                200,
                "image/png",
                b"\x89PNG\r\n\x1a\nlogo-bytes",
            ),
            txn(
                1,
                "GET",
                "example.com",
                "/index.html",
                200,
                "text/html",
                b"<html><body>home</body></html>",
            ),
            txn(
                2,
                "POST",
                "api.example.com",
                "/api/login",
                200,
                "application/json",
                br#"{"token":"abc","user":"sam"}"#,
            ),
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

    for w in rows.windows(2) {
        assert!(
            w[0].priority >= w[1].priority,
            "rows not ordered by priority DESC"
        );
    }
    let pos = |seq: usize| rows.iter().position(|r| r.seq == seq).unwrap();
    assert!(pos(2) < pos(0), "api POST should outrank the static asset");
    assert_eq!(rows[pos(2)].resource_class, "api_candidate");
    assert_eq!(rows[pos(0)].resource_class, "static_asset");

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
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("store.db");

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

#[test]
fn identical_bytes_different_content_type_do_not_collide() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("store.db");
    let bytes = b"ambiguous-payload";
    let session = Session {
        source: SessionSource::Live,
        transactions: vec![
            txn(
                0,
                "GET",
                "a.example.com",
                "/x",
                200,
                "application/json",
                bytes,
            ),
            txn(1, "GET", "b.example.com", "/y", 200, "text/plain", bytes),
        ],
    };
    {
        let store = TrafficStore::open(Some(&db_path)).unwrap();
        store
            .ingest("live", "live", None, None, &session, 65536)
            .unwrap();
        let a = store.get("live", 0).unwrap().unwrap();
        let b = store.get("live", 1).unwrap().unwrap();
        assert_eq!(
            a.response.unwrap().raw.content_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(
            b.response.unwrap().raw.content_type.as_deref(),
            Some("text/plain")
        );
    }
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let n: i64 = conn
        .query_row("SELECT count(*) FROM bodies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2, "different content-types must not share one body row");
}

#[test]
fn evict_retains_at_least_the_newest() {
    let store = TrafficStore::open(None).unwrap();
    let session = sample_session();
    store
        .ingest(
            "file:one",
            "file",
            Some("/one.har"),
            Some("/one.har:1:1"),
            &session,
            65536,
        )
        .unwrap();
    store.evict_file_captures(0).unwrap();
    assert_eq!(
        store.entry_count("file:one").unwrap(),
        4,
        "the just-ingested file capture must survive eviction"
    );
}
