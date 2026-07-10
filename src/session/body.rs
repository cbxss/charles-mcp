use std::io::Read;

use super::{Body, RawBody, charset_from_content_type, grpc, protobuf};

#[derive(Default, Clone, Copy)]
pub struct DecodeOptions<'a> {
    #[cfg(feature = "proto")]
    pub pool: Option<&'a protobuf::ProtoPool>,
    pub proto_type: Option<&'a str>,
}

pub fn decode(raw: &RawBody, max_bytes: usize) -> Body {
    decode_with(raw, max_bytes, &DecodeOptions::default())
}

pub fn ws_frame_text(payload: &RawBody, max_bytes: usize) -> String {
    match decode(payload, max_bytes) {
        Body::Text { text, .. } => text,
        _ => protobuf::try_decode_to_tree(&payload.bytes).unwrap_or_default(),
    }
}

pub fn decode_with(raw: &RawBody, max_bytes: usize, opts: &DecodeOptions) -> Body {
    if !raw.captured {
        return Body::NotCaptured;
    }
    if raw.bytes.is_empty() {
        return Body::Empty;
    }

    let decompressed = decompress(&raw.bytes, raw.content_encoding.as_deref());
    let bytes: &[u8] = decompressed.as_deref().unwrap_or(&raw.bytes);

    if let Some(ct) = raw.content_type.as_deref() {
        let ctl = ct.to_ascii_lowercase();
        if (grpc::is_grpc_ct(&ctl) || grpc::is_protobuf_ct(&ctl))
            && let Some(body) = decode_protobuf(bytes, raw, &ctl, max_bytes, opts)
        {
            return body;
        }
    }

    if is_binary(bytes, raw.content_type.as_deref()) {
        let sample_len = bytes.len().min(64);
        return Body::Binary {
            bytes_len: bytes.len() as u64,
            sample_hex: hex(&bytes[..sample_len]),
            truncated: bytes.len() > sample_len,
        };
    }

    let charset = raw
        .declared_charset
        .clone()
        .or_else(|| charset_from_content_type(raw.content_type.as_deref()))
        .unwrap_or_else(|| "utf-8".to_string());

    let enc = encoding_rs::Encoding::for_label(charset.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = enc.decode(bytes);
    let full = maybe_pretty_json(&text, raw.content_type.as_deref());
    let original_len = full.len() as u64;

    let truncated = full.len() > max_bytes;
    let text = if truncated {
        truncate_str(&full, max_bytes)
    } else {
        full
    };

    Body::Text {
        text,
        charset,
        truncated,
        original_len,
    }
}

fn decode_protobuf(
    bytes: &[u8],
    raw: &RawBody,
    ctl: &str,
    max_bytes: usize,
    opts: &DecodeOptions,
) -> Option<Body> {
    let payloads: Vec<Vec<u8>> = if grpc::is_grpc_ct(ctl) {
        let frames = grpc::split_frames(bytes, grpc::is_grpc_web_text_ct(ctl))?;
        let mut out = Vec::new();
        for f in &frames {
            if f.flags & 0x80 != 0 {
                continue;
            }
            out.push(
                grpc::decompress_frame(f, raw.grpc_encoding.as_deref())
                    .unwrap_or_else(|| f.data.clone()),
            );
        }
        if out.is_empty() {
            return None;
        }
        out
    } else {
        vec![bytes.to_vec()]
    };

    let count = payloads.len();
    let mut tree = String::new();
    let mut named = false;
    for (i, payload) in payloads.iter().enumerate() {
        let (rendered, was_named) = protobuf::try_decode(payload, opts)?;
        named |= was_named;
        if count > 1 {
            tree.push_str(&format!("── message {} ──\n", i + 1));
        }
        tree.push_str(&rendered);
        if !tree.ends_with('\n') {
            tree.push('\n');
        }
    }

    let original_len = tree.len() as u64;
    let truncated = tree.len() > max_bytes;
    if truncated {
        tree = truncate_str(&tree, max_bytes);
    }
    Some(Body::Protobuf {
        tree,
        message_count: count,
        named,
        truncated,
        original_len,
    })
}

fn decompress(bytes: &[u8], enc: Option<&str>) -> Option<Vec<u8>> {
    let enc = enc?.trim().to_ascii_lowercase();
    match enc.as_str() {
        "gzip" | "x-gzip" => {
            if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
                return None;
            }
            read_capped(flate2::read::GzDecoder::new(bytes))
        }
        "deflate" => read_capped(flate2::read::ZlibDecoder::new(bytes))
            .or_else(|| read_capped(flate2::read::DeflateDecoder::new(bytes))),
        "br" => read_capped(brotli::Decompressor::new(bytes, 4096)),
        _ => None,
    }
}

const MAX_DECOMPRESSED: u64 = 64 * 1024 * 1024;

fn read_capped<R: Read>(reader: R) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    reader.take(MAX_DECOMPRESSED).read_to_end(&mut out).ok()?;
    (!out.is_empty()).then_some(out)
}

fn is_binary(bytes: &[u8], content_type: Option<&str>) -> bool {
    if bytes.contains(&0) {
        return true;
    }
    if let Some(ct) = content_type {
        let ct = ct.to_ascii_lowercase();
        if is_textual_ct(&ct) {
            return false;
        }
        if is_binary_ct(&ct) {
            return true;
        }
    }
    let sample = &bytes[..bytes.len().min(2048)];
    std::str::from_utf8(sample).is_err()
}

fn is_textual_ct(ct: &str) -> bool {
    ct.starts_with("text/")
        || ct.contains("json")
        || ct.contains("xml")
        || ct.contains("javascript")
        || ct.contains("ecmascript")
        || ct.contains("html")
        || ct.contains("x-www-form-urlencoded")
        || ct.contains("csv")
        || ct.contains("graphql")
}

fn is_binary_ct(ct: &str) -> bool {
    ct.starts_with("image/")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct.starts_with("font/")
        || ct.contains("octet-stream")
        || ct.contains("protobuf")
        || ct.contains("grpc")
        || ct.contains("zip")
        || ct.contains("gzip")
        || ct.contains("pdf")
        || ct.contains("wasm")
}

fn maybe_pretty_json(text: &str, content_type: Option<&str>) -> String {
    let looks_json = content_type
        .map(|c| c.to_ascii_lowercase().contains("json"))
        .unwrap_or(false)
        || matches!(text.trim_start().chars().next(), Some('{') | Some('['));
    if looks_json
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(text)
        && let Ok(pretty) = serde_json::to_string_pretty(&v)
    {
        return pretty;
    }
    text.to_string()
}

fn truncate_str(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
