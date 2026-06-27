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

fn frame(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![flags];
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

#[test]
fn grpc_single_frame() {
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
    let mut body = frame(0, &[0x08, 0x2a]);
    body.extend(frame(0, &[0x10, 0x07]));
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
    match body::decode(
        &raw(vec![0xff, 0xff, 0xff], "application/x-protobuf"),
        1 << 16,
    ) {
        Body::Binary { .. } => {}
        other => panic!("expected Binary hex fallback, got {other:?}"),
    }
}
