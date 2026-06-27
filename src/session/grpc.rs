//! gRPC frame splitting + content-type classification.
//!
//! gRPC bodies are a sequence of length-prefixed frames; gRPC-Web additionally
//! base64-wraps the whole stream ("grpc-web-text") and appends a trailers frame.

use std::io::Read;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Cap decompressed frame output to guard against decompression bombs.
const MAX_DECOMPRESSED: u64 = 64 * 1024 * 1024;

/// True for gRPC content-types (application/grpc, +proto, grpc-web, grpc-web-text).
///
/// `ct` is expected to already be lowercased.
pub fn is_grpc_ct(ct: &str) -> bool {
    ct.starts_with("application/grpc")
}

/// True for the base64-wrapped gRPC-Web text transport.
///
/// `ct` is expected to already be lowercased.
pub fn is_grpc_web_text_ct(ct: &str) -> bool {
    ct.starts_with("application/grpc-web-text")
}

/// True for a bare protobuf body (application/x-protobuf, `*+protobuf`, application/protobuf).
///
/// gRPC content-types also contain "protobuf"-adjacent tokens but are handled
/// separately, so they are explicitly excluded here.
pub fn is_protobuf_ct(ct: &str) -> bool {
    if is_grpc_ct(ct) {
        return false;
    }
    ct == "application/x-protobuf" || ct == "application/protobuf" || ct.ends_with("+protobuf")
}

/// One gRPC length-prefixed frame: `[1B flags][4B BE len][len bytes]`.
///
/// `flags` bit0 (`0x01`) marks a compressed payload; bit7 (`0x80`) marks the
/// gRPC-Web trailers frame, whose payload is trailer text rather than protobuf.
pub struct Frame {
    pub flags: u8,
    pub data: Vec<u8>,
}

/// Split a (HTTP-decompressed) gRPC body into frames; `web_text` base64-decodes
/// first. `None` if the stream doesn't tile cleanly into frames.
pub fn split_frames(body: &[u8], web_text: bool) -> Option<Vec<Frame>> {
    let decoded;
    let bytes: &[u8] = if web_text {
        decoded = BASE64.decode(body).ok()?;
        &decoded
    } else {
        body
    };

    let mut frames = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        // Need at least the 1-byte flags + 4-byte length header.
        if bytes.len() - pos < 5 {
            return None;
        }
        let flags = bytes[pos];
        let len = u32::from_be_bytes([
            bytes[pos + 1],
            bytes[pos + 2],
            bytes[pos + 3],
            bytes[pos + 4],
        ]) as usize;
        let data_start = pos + 5;
        let data_end = data_start.checked_add(len)?;
        if data_end > bytes.len() {
            return None;
        }
        frames.push(Frame {
            flags,
            data: bytes[data_start..data_end].to_vec(),
        });
        pos = data_end;
    }

    Some(frames)
}

/// Decompress one frame's payload per `grpc-encoding` if its compressed bit is set.
pub fn decompress_frame(frame: &Frame, grpc_encoding: Option<&str>) -> Option<Vec<u8>> {
    // Compressed bit (0x01) clear => payload is already plain.
    if frame.flags & 0x01 == 0 {
        return Some(frame.data.clone());
    }

    match grpc_encoding {
        Some("gzip") => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(frame.data.as_slice())
                .take(MAX_DECOMPRESSED)
                .read_to_end(&mut out)
                .ok()?;
            Some(out)
        }
        Some("deflate") => {
            // gRPC "deflate" is zlib-wrapped per spec, but some implementations
            // emit raw DEFLATE; try zlib first, then fall back to raw.
            let mut out = Vec::new();
            if flate2::read::ZlibDecoder::new(frame.data.as_slice())
                .take(MAX_DECOMPRESSED)
                .read_to_end(&mut out)
                .is_ok()
            {
                return Some(out);
            }
            out.clear();
            flate2::read::DeflateDecoder::new(frame.data.as_slice())
                .take(MAX_DECOMPRESSED)
                .read_to_end(&mut out)
                .ok()?;
            Some(out)
        }
        // "identity" or unspecified: nothing to do (compressed bit notwithstanding).
        Some("identity") | None => Some(frame.data.clone()),
        // Unknown encoding we can't handle.
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn classifies_grpc_content_types() {
        assert!(is_grpc_ct("application/grpc"));
        assert!(is_grpc_ct("application/grpc+proto"));
        assert!(is_grpc_ct("application/grpc-web"));
        assert!(is_grpc_ct("application/grpc-web+proto"));
        assert!(is_grpc_ct("application/grpc-web-text"));
        assert!(is_grpc_ct("application/grpc-web-text+proto"));
        assert!(!is_grpc_ct("application/protobuf"));
        assert!(!is_grpc_ct("application/json"));
    }

    #[test]
    fn classifies_grpc_web_text() {
        assert!(is_grpc_web_text_ct("application/grpc-web-text"));
        assert!(is_grpc_web_text_ct("application/grpc-web-text+proto"));
        assert!(!is_grpc_web_text_ct("application/grpc-web"));
        assert!(!is_grpc_web_text_ct("application/grpc"));
    }

    #[test]
    fn classifies_protobuf() {
        assert!(is_protobuf_ct("application/x-protobuf"));
        assert!(is_protobuf_ct("application/protobuf"));
        assert!(is_protobuf_ct("application/vnd.foo+protobuf"));
        // gRPC types are handled separately, never reported as plain protobuf.
        assert!(!is_protobuf_ct("application/grpc+proto"));
        assert!(!is_protobuf_ct("application/grpc-web-text+proto"));
        assert!(!is_protobuf_ct("application/json"));
    }

    #[test]
    fn grpc_and_protobuf_are_disjoint_for_grpc_proto() {
        let ct = "application/grpc+proto";
        assert!(is_grpc_ct(ct));
        assert!(!is_protobuf_ct(ct));
    }

    #[test]
    fn splits_single_frame() {
        let body = [0x00, 0x00, 0x00, 0x00, 0x03, 0xAA, 0xBB, 0xCC];
        let frames = split_frames(&body, false).expect("should tile");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].flags, 0x00);
        assert_eq!(frames[0].data, vec![0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn splits_two_concatenated_frames() {
        let body = [
            0x00, 0x00, 0x00, 0x00, 0x02, 0x01, 0x02, // frame 1
            0x00, 0x00, 0x00, 0x00, 0x01, 0xFF, // frame 2
        ];
        let frames = split_frames(&body, false).expect("should tile");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, vec![0x01, 0x02]);
        assert_eq!(frames[1].data, vec![0xFF]);
    }

    #[test]
    fn keeps_trailers_frame() {
        // Trailers frame: flags 0x80, payload is ASCII trailer text.
        let body = [0x80, 0x00, 0x00, 0x00, 0x04, b'g', b'r', b'p', b'c'];
        let frames = split_frames(&body, false).expect("should tile");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].flags, 0x80);
        assert_eq!(frames[0].data, b"grpc".to_vec());
    }

    #[test]
    fn rejects_non_tiling_body() {
        // Declares 9 bytes of payload but only 2 follow.
        let body = [0x00, 0x00, 0x00, 0x00, 0x09, 0x01, 0x02];
        assert!(split_frames(&body, false).is_none());
    }

    #[test]
    fn rejects_truncated_header() {
        // Trailing leftover bytes shorter than a frame header.
        let body = [
            0x00, 0x00, 0x00, 0x00, 0x01, 0xFF, // one complete frame
            0x00, 0x00, // dangling partial header
        ];
        assert!(split_frames(&body, false).is_none());
    }

    #[test]
    fn rejects_invalid_base64_web_text() {
        assert!(split_frames(b"not valid base64!!!", true).is_none());
    }

    #[test]
    fn splits_grpc_web_text_frame() {
        let raw = [0x00, 0x00, 0x00, 0x00, 0x03, 0xAA, 0xBB, 0xCC];
        let encoded = BASE64.encode(raw);
        let frames = split_frames(encoded.as_bytes(), true).expect("should decode + tile");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].flags, 0x00);
        assert_eq!(frames[0].data, vec![0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn decompress_uncompressed_frame_is_identity() {
        let frame = Frame {
            flags: 0x00,
            data: vec![1, 2, 3],
        };
        assert_eq!(decompress_frame(&frame, Some("gzip")), Some(vec![1, 2, 3]));
    }

    #[test]
    fn decompress_gzip_frame() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"hello").unwrap();
        let payload = enc.finish().unwrap();
        let frame = Frame {
            flags: 0x01,
            data: payload,
        };
        assert_eq!(
            decompress_frame(&frame, Some("gzip")),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn decompress_deflate_zlib_frame() {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"hello").unwrap();
        let payload = enc.finish().unwrap();
        let frame = Frame {
            flags: 0x01,
            data: payload,
        };
        assert_eq!(
            decompress_frame(&frame, Some("deflate")),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn decompress_deflate_raw_frame() {
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"hello").unwrap();
        let payload = enc.finish().unwrap();
        let frame = Frame {
            flags: 0x01,
            data: payload,
        };
        assert_eq!(
            decompress_frame(&frame, Some("deflate")),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn decompress_bad_gzip_returns_none() {
        let frame = Frame {
            flags: 0x01,
            data: vec![0x00, 0x01, 0x02, 0x03],
        };
        assert!(decompress_frame(&frame, Some("gzip")).is_none());
    }
}
