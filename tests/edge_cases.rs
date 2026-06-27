use charles_mcp::session::{
    HttpMessage, RawBody, Session, SessionSource, Transaction, body, chlsj,
    looks_like_schema_mismatch,
};
use charles_mcp::store::{StoreFilters, TrafficStore};

fn filters(limit: usize) -> StoreFilters {
    StoreFilters {
        limit,
        ..Default::default()
    }
}

// ---- chlsj parser edge cases -------------------------------------------------

#[test]
fn empty_array_parses_to_nothing_and_is_not_a_mismatch() {
    let txns = chlsj::parse(b"[]").expect("empty array parses");
    assert!(txns.is_empty());
    // An empty session must NOT be flagged as a schema mismatch.
    assert!(!looks_like_schema_mismatch(&txns));
}

#[test]
fn whitespace_and_minus_one_everywhere_does_not_crash() {
    let json = br#"  [
      { "host": "h.example.com", "scheme": "https", "method": "GET", "path": "/p",
        "actualPort": -1, "port": -1,
        "request":  { "sizes": { "body": -1 }, "body": { "size": -1 } },
        "response": { "status": -1, "sizes": { "body": -1 }, "body": { "size": -1 } } }
    ]  "#;
    let txns = chlsj::parse(json).expect("parses despite -1 sentinels + leading ws");
    assert_eq!(txns.len(), 1);
    assert_eq!(txns[0].status, None, "-1 status maps to None");
}

#[test]
fn invalid_base64_body_is_lenient_not_fatal() {
    let json = br#"[
      { "host": "h", "method": "GET", "path": "/",
        "response": { "status": 200,
          "header": { "headers": [ { "name": "Content-Type", "value": "application/octet-stream" } ] },
          "body": { "encoded": "@@@not base64@@@" } } }
    ]"#;
    let txns = chlsj::parse(json).expect("invalid base64 must not fail the parse");
    let resp = txns[0].response.as_ref().unwrap();
    // Lenient decode yields empty bytes; the message is still marked captured.
    assert!(resp.raw.bytes.is_empty());
    assert!(resp.raw.captured);
    // Decoding an empty captured body renders as Empty, not garbage.
    assert_eq!(
        body::decode(&resp.raw, 4096),
        charles_mcp::session::Body::Empty
    );
}

#[test]
fn error_message_wins_over_default_200_status() {
    let json = br#"[
      { "host": "h", "method": "CONNECT", "path": "",
        "errorMessage": "SSL handshake with client failed",
        "response": { "status": 200 } }
    ]"#;
    let txns = chlsj::parse(json).unwrap();
    assert_eq!(
        txns[0].error.as_deref(),
        Some("SSL handshake with client failed")
    );
    assert_eq!(
        txns[0].status, None,
        "a failed connection's bogus 200 is dropped"
    );
}

#[test]
fn duplicate_headers_are_preserved_in_order() {
    let json = br#"[
      { "host": "h", "method": "GET", "path": "/",
        "request": { "header": { "headers": [
          { "name": "Set-Cookie", "value": "a=1" },
          { "name": "Set-Cookie", "value": "b=2" } ] } } }
    ]"#;
    let txns = chlsj::parse(json).unwrap();
    let h = &txns[0].request.headers;
    assert_eq!(h.len(), 2);
    assert_eq!(h[0], ("Set-Cookie".into(), "a=1".into()));
    assert_eq!(h[1], ("Set-Cookie".into(), "b=2".into()));
}

#[test]
fn one_all_empty_entry_is_flagged_as_schema_mismatch() {
    // A single object with no recognized fields → host+method empty → mismatch.
    let txns = chlsj::parse(br#"[{}]"#).unwrap();
    assert!(looks_like_schema_mismatch(&txns));
}

// ---- store edge cases --------------------------------------------------------

fn txn(seq: usize, host: &str, status: Option<u16>, body: &[u8]) -> Transaction {
    Transaction {
        index: seq,
        scheme: "https".into(),
        host: host.into(),
        method: "GET".into(),
        path: "/".into(),
        url: format!("https://{host}/"),
        status,
        mime: Some("application/json".into()),
        response: Some(HttpMessage {
            headers: vec![],
            raw: RawBody {
                bytes: body.to_vec(),
                content_type: Some("application/json".into()),
                captured: !body.is_empty(),
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

fn live(transactions: Vec<Transaction>) -> Session {
    Session {
        source: SessionSource::Live,
        transactions,
    }
}

#[test]
fn ingesting_an_empty_session_is_well_defined() {
    let store = TrafficStore::open(None).unwrap();
    let cap = store
        .ingest("live", "live", None, None, &live(vec![]), 4096)
        .unwrap();
    assert_eq!(cap.entry_count, 0);
    let (rows, total) = store.list("live", &filters(50)).unwrap();
    assert!(rows.is_empty());
    assert_eq!(total, 0);
    assert_eq!(store.stats("live").unwrap().total, 0);
    assert_eq!(store.entry_count("live").unwrap(), 0);
    assert!(store.get("live", 0).unwrap().is_none());
}

#[test]
fn reingest_replaces_wholesale_and_bumps_generation() {
    let store = TrafficStore::open(None).unwrap();
    let first = live(vec![
        txn(0, "a.example.com", Some(200), b"{\"a\":1}"),
        txn(1, "b.example.com", Some(200), b"{\"b\":2}"),
        txn(2, "c.example.com", Some(200), b"{\"c\":3}"),
    ]);
    let c0 = store
        .ingest("live", "live", None, None, &first, 4096)
        .unwrap();
    assert_eq!(c0.generation, 0);
    assert_eq!(c0.entry_count, 3);

    // A smaller snapshot replaces the old one entirely (no merge).
    let second = live(vec![txn(0, "z.example.com", Some(204), b"")]);
    let c1 = store
        .ingest("live", "live", None, None, &second, 4096)
        .unwrap();
    assert_eq!(c1.generation, 1, "generation bumps on re-ingest");
    assert_eq!(c1.entry_count, 1);
    let (rows, total) = store.list("live", &filters(50)).unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].host, "z.example.com");
    // The old seq 2 is gone, not lingering from the larger first snapshot.
    assert!(store.get("live", 2).unwrap().is_none());
}

#[test]
fn get_out_of_range_seq_is_none_not_an_error() {
    let store = TrafficStore::open(None).unwrap();
    store
        .ingest(
            "live",
            "live",
            None,
            None,
            &live(vec![txn(0, "h", Some(200), b"x")]),
            4096,
        )
        .unwrap();
    assert!(store.get("live", 99).unwrap().is_none());
    // An unknown capture id also yields nothing rather than erroring.
    assert!(store.get("does-not-exist", 0).unwrap().is_none());
    assert_eq!(store.entry_count("does-not-exist").unwrap(), 0);
}

#[test]
fn fts_with_no_match_returns_empty() {
    let store = TrafficStore::open(None).unwrap();
    store
        .ingest(
            "live",
            "live",
            None,
            None,
            &live(vec![txn(0, "h", Some(200), b"{\"k\":1}")]),
            4096,
        )
        .unwrap();
    let hits = store.search_fts("live", "zzz_no_such_token", 10).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn entry_with_no_response_round_trips_as_none() {
    let store = TrafficStore::open(None).unwrap();
    let mut t = txn(0, "h", None, b"");
    t.response = None; // request issued, no response captured
    store
        .ingest("live", "live", None, None, &live(vec![t]), 4096)
        .unwrap();
    let got = store.get("live", 0).unwrap().unwrap();
    assert!(got.response.is_none());
    assert!(!got.request.raw.captured);
}

#[test]
fn empty_but_captured_body_reconstructs_without_a_body_row() {
    let store = TrafficStore::open(None).unwrap();
    // captured=true with empty bytes → no body stored, but the flag round-trips.
    let mut t = txn(0, "h", Some(204), b"");
    t.response.as_mut().unwrap().raw.captured = true;
    store
        .ingest("live", "live", None, None, &live(vec![t]), 4096)
        .unwrap();
    let got = store.get("live", 0).unwrap().unwrap();
    let resp = got.response.unwrap();
    assert!(resp.raw.bytes.is_empty());
    assert!(resp.raw.captured);
}
