//! End-to-end: protobuf/gRPC bodies flow through `body::decode` → `Body::Protobuf`.

use base64::Engine as _;
use charles_mcp::session::{Body, RawBody, body};

fn raw(bytes: Vec<u8>, ct: &str) -> RawBody {
    RawBody {
        bytes,
        content_type: Some(ct.to_string()),
        captured: true,
        ..Default::default()
    }
}

/// A gRPC frame: 1-byte flags + 4-byte BE length + payload.
fn frame(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![flags];
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

// The real-captured x-protobuf body test lives in tests/real_capture.rs
// (gitignored) to keep real traffic out of git.

#[test]
fn grpc_single_frame() {
    // protobuf message: field 1 = varint 150 (08 96 01)
    let body = frame(0, &[0x08, 0x96, 0x01]);
    match body::decode(&raw(body, "application/grpc+proto"), 1 << 16) {
        Body::Protobuf {
            tree,
            message_count,
            ..
        } => {
            assert_eq!(message_count, 1);
            assert!(tree.contains("1: 150"));
        }
        other => panic!("expected Protobuf, got {other:?}"),
    }
}

#[test]
fn grpc_two_frames() {
    let mut body = frame(0, &[0x08, 0x2a]); // field1 = 42
    body.extend(frame(0, &[0x10, 0x07])); // field2 = 7
    match body::decode(&raw(body, "application/grpc"), 1 << 16) {
        Body::Protobuf { message_count, .. } => assert_eq!(message_count, 2),
        other => panic!("expected Protobuf, got {other:?}"),
    }
}

#[test]
fn grpc_web_text_base64() {
    let inner = frame(0, &[0x08, 0x96, 0x01]);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&inner);
    match body::decode(
        &raw(encoded.into_bytes(), "application/grpc-web-text"),
        1 << 16,
    ) {
        Body::Protobuf {
            tree,
            message_count,
            ..
        } => {
            assert_eq!(message_count, 1);
            assert!(tree.contains("1: 150"));
        }
        other => panic!("expected Protobuf, got {other:?}"),
    }
}

#[test]
fn malformed_protobuf_falls_back_to_binary() {
    // content-type says protobuf, but the bytes are not a clean message.
    match body::decode(
        &raw(vec![0xff, 0xff, 0xff], "application/x-protobuf"),
        1 << 16,
    ) {
        Body::Binary { .. } => {}
        other => panic!("expected Binary hex fallback, got {other:?}"),
    }
}
