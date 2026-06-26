use std::io::Write;

use charles_mcp::session::{Body, RawBody, body};

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn brotli_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
        w.write_all(data).unwrap();
    }
    out
}

fn raw(bytes: Vec<u8>, ce: Option<&str>, ct: Option<&str>) -> RawBody {
    RawBody {
        bytes,
        content_encoding: ce.map(str::to_string),
        content_type: ct.map(str::to_string),
        declared_charset: None,
        was_base64_wrapped: false,
        captured: true,
    }
}

#[test]
fn decodes_gzip() {
    let r = raw(gzip(b"hello gzip"), Some("gzip"), Some("text/plain"));
    match body::decode(&r, 8192) {
        Body::Text { text, .. } => assert_eq!(text, "hello gzip"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn decodes_deflate_zlib() {
    let r = raw(zlib(b"hello deflate"), Some("deflate"), Some("text/plain"));
    match body::decode(&r, 8192) {
        Body::Text { text, .. } => assert_eq!(text, "hello deflate"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn decodes_brotli() {
    let r = raw(
        brotli_compress(b"hello brotli"),
        Some("br"),
        Some("text/plain"),
    );
    match body::decode(&r, 8192) {
        Body::Text { text, .. } => assert_eq!(text, "hello brotli"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn already_decoded_guard_skips_decompress() {
    // Encoding claims gzip but bytes are plain text (Charles pre-decoded it).
    let r = raw(
        b"plain not gzipped".to_vec(),
        Some("gzip"),
        Some("text/plain"),
    );
    match body::decode(&r, 8192) {
        Body::Text { text, .. } => assert_eq!(text, "plain not gzipped"),
        o => panic!("{o:?}"),
    }
}

#[test]
fn detects_binary_via_nul() {
    let r = raw(vec![0x00, 0x01, 0x02, 0xff], None, None);
    match body::decode(&r, 8192) {
        Body::Binary {
            bytes_len,
            sample_hex,
            ..
        } => {
            assert_eq!(bytes_len, 4);
            assert!(sample_hex.starts_with("000102"));
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn detects_binary_via_content_type() {
    let r = raw(b"\x89PNGdata".to_vec(), None, Some("image/png"));
    assert!(matches!(body::decode(&r, 8192), Body::Binary { .. }));
}

#[test]
fn pretty_prints_json() {
    let r = raw(
        br#"{"a":1,"b":[2,3]}"#.to_vec(),
        None,
        Some("application/json"),
    );
    match body::decode(&r, 8192) {
        Body::Text { text, .. } => {
            assert!(text.contains('\n'), "expected pretty-printed JSON: {text}");
            assert!(text.contains("\"a\": 1"));
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn truncates_long_text() {
    let big = "x".repeat(20_000);
    let r = raw(big.into_bytes(), None, Some("text/plain"));
    match body::decode(&r, 100) {
        Body::Text {
            text,
            truncated,
            original_len,
            ..
        } => {
            assert!(truncated);
            assert!(text.len() <= 100);
            assert_eq!(original_len, 20_000);
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn empty_and_uncaptured() {
    let mut r = raw(Vec::new(), None, Some("text/plain"));
    assert_eq!(body::decode(&r, 8192), Body::Empty);
    r.captured = false;
    assert_eq!(body::decode(&r, 8192), Body::NotCaptured);
}
