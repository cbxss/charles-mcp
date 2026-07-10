use charles_mcp::session::{
    HttpMessage, RawBody, Session, SessionSource, Transaction, WsDirection, WsMessage, WsOpcode,
};
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

fn txn_opt(
    method: &str,
    host: &str,
    path: &str,
    status: Option<u16>,
    body: Option<&[u8]>,
) -> Transaction {
    Transaction {
        scheme: "https".into(),
        host: host.into(),
        method: method.into(),
        path: path.into(),
        url: format!("https://{host}{path}"),
        status,
        response_size: body.map(|b| b.len() as u64),
        request: HttpMessage {
            headers: vec![],
            raw: RawBody::default(),
        },
        response: body.map(|b| HttpMessage {
            headers: vec![],
            raw: RawBody {
                bytes: b.to_vec(),
                captured: true,
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

fn live_session(txns: Vec<Transaction>) -> Session {
    Session {
        source: SessionSource::Live,
        transactions: txns,
    }
}

fn ingest_live(store: &TrafficStore, session: &Session) -> charles_mcp::store::CaptureRef {
    store
        .ingest_incremental("live", "live", Some("live"), None, session, 65536)
        .unwrap()
}

#[test]
fn incremental_appends_new_entries_with_stable_seqs() {
    let store = TrafficStore::open(None).unwrap();
    let s1 = live_session(vec![
        txn_opt("GET", "a.com", "/1", Some(200), Some(b"one")),
        txn_opt("GET", "b.com", "/2", Some(200), Some(b"two")),
    ]);
    assert_eq!(ingest_live(&store, &s1).entry_count, 2);

    let s2 = live_session(vec![
        txn_opt("GET", "a.com", "/1", Some(200), Some(b"one")),
        txn_opt("GET", "b.com", "/2", Some(200), Some(b"two")),
        txn_opt("GET", "c.com", "/3", Some(200), Some(b"three")),
    ]);
    let c2 = ingest_live(&store, &s2);
    assert_eq!(c2.entry_count, 3);
    assert_eq!(c2.generation, 1);

    assert_eq!(store.get("live", 0).unwrap().unwrap().host, "a.com");
    assert_eq!(store.get("live", 1).unwrap().unwrap().host, "b.com");
    assert_eq!(
        store.get("live", 2).unwrap().unwrap().host,
        "c.com",
        "the new arrival gets the next seq"
    );
}

#[test]
fn incremental_updates_completed_response_in_place() {
    let store = TrafficStore::open(None).unwrap();
    ingest_live(
        &store,
        &live_session(vec![txn_opt("POST", "api.com", "/login", None, None)]),
    );
    assert!(store.get("live", 0).unwrap().unwrap().response.is_none());

    let c2 = ingest_live(
        &store,
        &live_session(vec![txn_opt(
            "POST",
            "api.com",
            "/login",
            Some(200),
            Some(br#"{"ok":true}"#),
        )]),
    );
    assert_eq!(c2.entry_count, 1, "same request completing must not duplicate");
    let done = store.get("live", 0).unwrap().unwrap();
    assert_eq!(done.status, Some(200));
    assert_eq!(
        done.response.unwrap().raw.bytes,
        br#"{"ok":true}"#.to_vec()
    );
}

#[test]
fn incremental_skips_unchanged_entries() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("s.db");
    let store = TrafficStore::open(Some(&db)).unwrap();
    let s = live_session(vec![
        txn_opt("GET", "a.com", "/1", Some(200), Some(b"one")),
        txn_opt("GET", "b.com", "/2", Some(200), Some(b"two")),
    ]);
    ingest_live(&store, &s);
    let rowid_of_seq0 = || -> i64 {
        rusqlite::Connection::open(&db)
            .unwrap()
            .query_row(
                "SELECT rowid FROM entries WHERE capture_id='live' AND seq=0",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let before = rowid_of_seq0();
    ingest_live(&store, &s);
    assert_eq!(
        before,
        rowid_of_seq0(),
        "an unchanged entry must not be re-inserted (rowid stays put)"
    );
}

#[test]
fn incremental_detects_session_reset() {
    let store = TrafficStore::open(None).unwrap();
    ingest_live(
        &store,
        &live_session(vec![
            txn_opt("GET", "a.com", "/1", Some(200), Some(b"one")),
            txn_opt("GET", "b.com", "/2", Some(200), Some(b"two")),
        ]),
    );

    let c2 = ingest_live(
        &store,
        &live_session(vec![txn_opt("GET", "x.com", "/new", Some(200), Some(b"fresh"))]),
    );
    assert_eq!(c2.entry_count, 1, "clearing the session drops the old entries");
    assert_eq!(store.get("live", 0).unwrap().unwrap().host, "x.com");
    assert!(
        store.get("live", 1).unwrap().is_none(),
        "old seq 1 must be gone after a reset"
    );
}

#[test]
fn mutation_does_not_shift_later_seqs() {
    let store = TrafficStore::open(None).unwrap();
    ingest_live(
        &store,
        &live_session(vec![
            txn_opt("GET", "a.com", "/a", Some(200), Some(b"a")),
            txn_opt("POST", "b.com", "/b", None, None),
        ]),
    );

    let c2 = ingest_live(
        &store,
        &live_session(vec![
            txn_opt("GET", "a.com", "/a", Some(200), Some(b"a")),
            txn_opt("POST", "b.com", "/b", Some(201), Some(b"done")),
            txn_opt("GET", "c.com", "/c", Some(200), Some(b"c")),
        ]),
    );
    assert_eq!(c2.entry_count, 3);
    let b = store.get("live", 1).unwrap().unwrap();
    assert_eq!(b.host, "b.com");
    assert_eq!(b.status, Some(201), "b completed in place, still at seq 1");
    assert_eq!(
        store.get("live", 2).unwrap().unwrap().host,
        "c.com",
        "the new arrival lands above all existing seqs"
    );
}

fn ws_frame(direction: WsDirection, opcode: WsOpcode, bytes: Vec<u8>) -> WsMessage {
    WsMessage {
        direction,
        opcode,
        payload: RawBody {
            bytes,
            captured: true,
            ..Default::default()
        },
    }
}

fn ws_txn() -> Transaction {
    let mut proto = vec![0x0a, 0x07];
    proto.extend_from_slice(b"wsproto");
    Transaction {
        scheme: "wss".into(),
        host: "socket.example.com".into(),
        method: "GET".into(),
        path: "/live".into(),
        url: "wss://socket.example.com/live".into(),
        status: Some(101),
        request: HttpMessage {
            headers: vec![],
            raw: RawBody::default(),
        },
        websocket: Some(vec![
            ws_frame(
                WsDirection::Received,
                WsOpcode::Text,
                b"hello socketsecret world".to_vec(),
            ),
            ws_frame(WsDirection::Sent, WsOpcode::Binary, proto),
        ]),
        ..Default::default()
    }
}

#[test]
fn fts_finds_websocket_frame_content() {
    let store = TrafficStore::open(None).unwrap();
    ingest_live(&store, &live_session(vec![ws_txn()]));

    let text_hits = store.search_fts("live", "socketsecret", 10).unwrap();
    assert!(
        text_hits.iter().any(|(seq, _)| *seq == 0),
        "a text frame's payload must be full-text searchable"
    );

    let proto_hits = store.search_fts("live", "wsproto", 10).unwrap();
    assert!(
        proto_hits.iter().any(|(seq, _)| *seq == 0),
        "a binary protobuf frame's decoded content must be searchable"
    );
}

#[test]
fn fts_matches_url_and_header_columns() {
    let store = TrafficStore::open(None).unwrap();
    ingest_live(&store, &sample_session());
    assert!(
        store
            .search_fts("live", "orders", 10)
            .unwrap()
            .iter()
            .any(|(seq, _)| *seq == 3),
        "a token from the URL (/api/orders) must be FTS-searchable"
    );
    assert!(
        !store.search_fts("live", "Accept", 10).unwrap().is_empty(),
        "a request header token must be FTS-searchable"
    );
}

#[test]
fn regex_search_covers_websocket_frames() {
    use charles_mcp::tools::inspect::{Matcher, search};
    let session = live_session(vec![ws_txn()]);
    let m = Matcher::build("socketsecret", false).unwrap();
    let hits = search(&session, &m, &[], 10);
    assert!(
        hits.iter().any(|h| h.field == "ws"),
        "the regex/substring path must reach ws frames, tagged 'ws'"
    );
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
