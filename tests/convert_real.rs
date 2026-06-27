//! Exercises `charles convert` for real (skipped when Charles isn't installed).
//! Catches plumbing bugs the synthetic tests can't — e.g. the output temp file
//! must NOT pre-exist, since `charles convert` refuses to overwrite it.

use std::path::Path;

use charles_mcp::config::Config;
use charles_mcp::session::{convert, sniff};
use clap::Parser;

fn cfg() -> Config {
    Config::parse_from(["charles-mcp"])
}

#[tokio::test]
async fn convert_har_to_chlsj() {
    let cfg = cfg();
    if !cfg.charles_bin.exists() {
        eprintln!(
            "skip: Charles binary not present at {}",
            cfg.charles_bin.display()
        );
        return;
    }
    let bytes = convert::convert_file(&cfg, Path::new("tests/fixtures/sample.har"), "chlsj")
        .await
        .expect("charles convert .har -> .chlsj");
    assert!(!bytes.is_empty());
    let txns = sniff::parse_bytes(bytes).expect("converted output parses");
    assert!(
        !txns.is_empty(),
        "expected transactions from the converted session"
    );
}

#[tokio::test]
async fn read_session_file_converts_har() {
    // read_session_file on a .har takes the direct path (no convert), but this
    // confirms the end-to-end Session shape via the public API.
    let cfg = cfg();
    let session = convert::read_session_file(&cfg, Path::new("tests/fixtures/sample.har"))
        .await
        .expect("read .har");
    assert_eq!(session.transactions.len(), 5);
}
