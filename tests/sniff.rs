use std::io::Write;

use charles_mcp::session::sniff::{Format, parse_bytes, sniff};

#[test]
fn detects_formats() {
    assert_eq!(sniff(b"[ {\"host\":\"x\"} ]"), Format::Chlsj);
    assert_eq!(sniff(b"  \n{\"log\":{}}"), Format::Har);
    assert_eq!(sniff(b"<?xml version=\"1.0\"?>"), Format::Xml);
    assert_eq!(sniff(&[0x1f, 0x8b, 0x08, 0x00]), Format::Gzip);
    assert_eq!(sniff(&[0x00, 0x01, 0x99]), Format::Native);
}

#[test]
fn parse_bytes_handles_har_and_chlsj() {
    let har = parse_bytes(include_bytes!("fixtures/sample.har").to_vec()).unwrap();
    assert_eq!(har.len(), 5);
    let chlsj = parse_bytes(include_bytes!("fixtures/sample.chlsj").to_vec()).unwrap();
    assert_eq!(chlsj.len(), 5);
}

#[test]
fn parse_bytes_inflates_gzip() {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(include_bytes!("fixtures/sample.har")).unwrap();
    let gzipped = e.finish().unwrap();
    let txns = parse_bytes(gzipped).unwrap();
    assert_eq!(txns.len(), 5);
}

#[test]
fn native_format_is_unsupported_without_conversion() {
    assert!(parse_bytes(vec![0x00, 0x01, 0x02]).is_err());
}
