//! Lazy decoding of a captured [`RawBody`] into a presentable [`Body`].
//!
//! Pipeline: (base64 already unwrapped at parse time) → content-encoding
//! decompress (with an already-decoded guard) → charset decode → binary
//! detection / JSON pretty-print / truncation.

use std::io::Read;

use super::{Body, RawBody, charset_from_content_type};

pub fn decode(raw: &RawBody, max_bytes: usize) -> Body {
    if !raw.captured {
        return Body::NotCaptured;
    }
    if raw.bytes.is_empty() {
        return Body::Empty;
    }

    let decompressed = decompress(&raw.bytes, raw.content_encoding.as_deref());
    let bytes: &[u8] = decompressed.as_deref().unwrap_or(&raw.bytes);

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

/// Decompress per `Content-Encoding`. Returns `None` when no decompression is
/// needed/possible — including the *already-decoded guard*: if the encoding
/// claims gzip but the bytes lack the gzip magic, assume Charles already
/// decoded it and leave the bytes untouched.
fn decompress(bytes: &[u8], enc: Option<&str>) -> Option<Vec<u8>> {
    let enc = enc?.trim().to_ascii_lowercase();
    match enc.as_str() {
        "gzip" | "x-gzip" => {
            if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
                return None; // already decoded
            }
            read_capped(flate2::read::GzDecoder::new(bytes))
        }
        "deflate" => {
            // Try zlib-wrapped first, then raw DEFLATE.
            read_capped(flate2::read::ZlibDecoder::new(bytes))
                .or_else(|| read_capped(flate2::read::DeflateDecoder::new(bytes)))
        }
        "br" => read_capped(brotli::Decompressor::new(bytes, 4096)),
        _ => None,
    }
}

/// Cap on decompressed output, to defuse decompression bombs.
const MAX_DECOMPRESSED: u64 = 64 * 1024 * 1024;

/// Read a decoder to end, bounded by [`MAX_DECOMPRESSED`]. `None` on error/empty.
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
    // Unknown content-type: sniff a prefix for UTF-8 validity.
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

/// Truncate a string to at most `max_bytes`, respecting char boundaries.
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
