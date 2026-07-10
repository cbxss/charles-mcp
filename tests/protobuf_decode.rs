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

#[cfg(feature = "proto")]
fn ping_pool() -> (tempfile::TempDir, charles_mcp::session::protobuf::ProtoPool) {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo.proto");
    let mut f = std::fs::File::create(&path).unwrap();
    write!(
        f,
        "syntax = \"proto3\";\npackage demo;\nmessage Ping {{ string msg = 1; int32 n = 2; }}\n"
    )
    .unwrap();
    f.flush().unwrap();
    let pool = charles_mcp::session::protobuf::ProtoPool::load_file(&path, None).unwrap();
    (dir, pool)
}

#[cfg(feature = "proto")]
#[test]
fn named_decode_raw_protobuf() {
    let (_dir, pool) = ping_pool();
    let opts = body::DecodeOptions {
        pool: Some(&pool),
        proto_type: Some("demo.Ping"),
    };
    let bytes = vec![0x0a, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x10, 0x07];
    match body::decode_with(&raw(bytes, "application/x-protobuf"), 1 << 16, &opts) {
        Body::Protobuf { tree, named, .. } => {
            assert!(named, "should be a named decode");
            assert!(tree.contains("hello"), "tree:\n{tree}");
            assert!(tree.contains("\"n\""), "tree:\n{tree}");
        }
        other => panic!("expected Protobuf, got {other:?}"),
    }
}

#[cfg(feature = "proto")]
#[test]
fn named_decode_grpc_frame_short_name() {
    let (_dir, pool) = ping_pool();
    let opts = body::DecodeOptions {
        pool: Some(&pool),
        proto_type: Some("Ping"),
    };
    let body = frame(0, &[0x0a, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x10, 0x07]);
    match body::decode_with(&raw(body, "application/grpc+proto"), 1 << 16, &opts) {
        Body::Protobuf { tree, named, .. } => {
            assert!(named, "short name within package should name-decode");
            assert!(tree.contains("hello"), "tree:\n{tree}");
        }
        other => panic!("expected Protobuf, got {other:?}"),
    }
}

#[cfg(feature = "proto")]
#[test]
fn load_file_resolves_imports_via_root_and_infers_local_message() {
    use std::io::Write as _;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("common.proto"),
        "syntax = \"proto3\";\npackage common;\nmessage Meta { string id = 1; }\n",
    )
    .unwrap();
    let sub = root.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let api = sub.join("api.proto");
    let mut f = std::fs::File::create(&api).unwrap();
    write!(
        f,
        "syntax = \"proto3\";\npackage demo;\nimport \"common.proto\";\nmessage Req {{ string name = 1; common.Meta meta = 2; }}\n"
    )
    .unwrap();
    f.flush().unwrap();

    let pool =
        charles_mcp::session::protobuf::ProtoPool::load_file(&api, Some(root.path())).unwrap();

    let bytes = vec![0x0a, 0x02, b'h', b'i', 0x12, 0x03, 0x0a, 0x01, b'x'];
    let opts = body::DecodeOptions {
        pool: Some(&pool),
        proto_type: None,
    };
    match body::decode_with(&raw(bytes, "application/x-protobuf"), 1 << 16, &opts) {
        Body::Protobuf { tree, named, .. } => {
            assert!(named, "should name-decode Req using the imported Meta type");
            assert!(tree.contains("hi"), "tree:\n{tree}");
            assert!(tree.contains("\"meta\"") && tree.contains("\"id\""), "tree:\n{tree}");
        }
        other => panic!("expected Protobuf, got {other:?}"),
    }
}

#[cfg(feature = "proto")]
#[test]
fn wrong_type_falls_back_to_schemaless() {
    let (_dir, pool) = ping_pool();
    let opts = body::DecodeOptions {
        pool: Some(&pool),
        proto_type: Some("demo.Nope"),
    };
    let bytes = vec![0x0a, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f];
    match body::decode_with(&raw(bytes, "application/x-protobuf"), 1 << 16, &opts) {
        Body::Protobuf { named, .. } => assert!(!named, "unknown type must not claim a named decode"),
        other => panic!("expected Protobuf, got {other:?}"),
    }
}
