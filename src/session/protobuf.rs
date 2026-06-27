//! Schemaless protobuf wire decoding (+ optional `.proto` named decoding).
//!
//! Contract: pure decode of protobuf *message* bytes (no gRPC framing here;
//! `body.rs` handles framing via `grpc`). [`try_decode`] returns the rendered
//! tree and whether it was decoded against a `.proto` type.
//!
//! The schemaless decoder (everything except [`ProtoPool`]) is std-only and
//! always compiled. Named `.proto` decoding lives behind `feature = "proto"`.

use crate::session::body::DecodeOptions;

/// Maximum nesting depth honoured while disambiguating length-delimited fields
/// as sub-messages — guards against pathological / adversarial recursion.
const MAX_DEPTH: usize = 100;

/// A decoded protobuf wire value. Length-delimited fields are disambiguated
/// (best-effort) into a nested [`WireValue::Message`], a [`WireValue::Str`], or
/// raw [`WireValue::Bytes`].
#[derive(Debug, Clone, PartialEq)]
pub enum WireValue {
    /// Wire type 0 — base-128 varint.
    Varint(u64),
    /// Wire type 1 — fixed 64-bit (little-endian), kept as raw bits.
    I64(u64),
    /// Wire type 5 — fixed 32-bit (little-endian), kept as raw bits.
    I32(u32),
    /// Wire type 2, decoded as a nested message.
    Message(Vec<Field>),
    /// Wire type 2, decoded as a UTF-8 string.
    Str(String),
    /// Wire type 2, opaque bytes.
    Bytes(Vec<u8>),
}

/// One decoded field: its number, raw wire type, and decoded value.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub number: u32,
    pub wire_type: u8,
    pub value: WireValue,
}

/// Decode protobuf message bytes to a printable tree. Returns `(rendered, named)`
/// — `named` is true if decoded against a `.proto` type via `opts`. Returns
/// `None` when the bytes are not a clean protobuf message (caller → hex).
pub fn try_decode(bytes: &[u8], opts: &DecodeOptions) -> Option<(String, bool)> {
    // Named decode (only when a pool + type are supplied and the type exists).
    #[cfg(feature = "proto")]
    if let (Some(ty), Some(pool)) = (opts.proto_type, opts.pool)
        && let Some(json) = pool.decode_named(ty, bytes)
    {
        return Some((json, true));
    }
    #[cfg(not(feature = "proto"))]
    let _ = opts;

    let tree = try_decode_to_tree(bytes)?;
    Some((tree, false))
}

/// Decode a whole message buffer. Accepts only if the bytes are consumed
/// *exactly* and every field number is in the valid range. `None` otherwise.
pub fn decode_message(bytes: &[u8]) -> Option<Vec<Field>> {
    decode_message_inner(bytes, 0)
}

/// Full schemaless pipeline: [`decode_message`] then [`render_tree`]. Requires
/// at least one top-level field (so non-protobuf input rarely renders).
pub fn try_decode_to_tree(bytes: &[u8]) -> Option<String> {
    let fields = decode_message(bytes)?;
    if fields.is_empty() {
        return None;
    }
    let mut out = String::new();
    render_tree(&mut out, &fields, 0);
    Some(out)
}

/// Render decoded fields as an indented tree (two spaces per level).
pub fn render_tree(out: &mut String, fields: &[Field], indent: usize) {
    let prefix = "  ".repeat(indent);
    for f in fields {
        match &f.value {
            WireValue::Varint(v) => {
                out.push_str(&format!("{prefix}{}: {v}\n", f.number));
            }
            WireValue::I64(v) => {
                let fl = f64::from_bits(*v);
                out.push_str(&format!(
                    "{prefix}{}: {v}  (i64; f64={fl}, 0x{v:016X})\n",
                    f.number
                ));
            }
            WireValue::I32(v) => {
                let fl = f32::from_bits(*v);
                out.push_str(&format!(
                    "{prefix}{}: {v}  (i32; f32={fl}, 0x{v:08X})\n",
                    f.number
                ));
            }
            WireValue::Str(s) => {
                out.push_str(&format!("{prefix}{}: {s:?}\n", f.number));
            }
            WireValue::Bytes(b) => {
                let shown: String = b.iter().take(16).map(|x| format!("{x:02X}")).collect();
                let ell = if b.len() > 16 { "…" } else { "" };
                out.push_str(&format!(
                    "{prefix}{}: 0x{shown}{ell}  ({} bytes)\n",
                    f.number,
                    b.len()
                ));
            }
            WireValue::Message(sub) => {
                out.push_str(&format!("{prefix}{} {{\n", f.number));
                render_tree(out, sub, indent + 1);
                out.push_str(&format!("{prefix}}}\n"));
            }
        }
    }
}

/// Decode a message buffer at the given recursion `depth`. See [`decode_message`].
fn decode_message_inner(bytes: &[u8], depth: usize) -> Option<Vec<Field>> {
    if depth > MAX_DEPTH {
        return None;
    }
    let mut fields = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let (tag, n) = read_varint(&bytes[pos..])?;
        pos += n;
        let fnum = tag >> 3;
        if !(1..=536_870_911).contains(&fnum) {
            return None;
        }
        let number = fnum as u32;
        let wire_type = (tag & 0x7) as u8;
        let value = match wire_type {
            0 => {
                let (v, n) = read_varint(&bytes[pos..])?;
                pos += n;
                WireValue::Varint(v)
            }
            1 => {
                if pos + 8 > bytes.len() {
                    return None;
                }
                let v = u64::from_le_bytes(bytes[pos..pos + 8].try_into().ok()?);
                pos += 8;
                WireValue::I64(v)
            }
            5 => {
                if pos + 4 > bytes.len() {
                    return None;
                }
                let v = u32::from_le_bytes(bytes[pos..pos + 4].try_into().ok()?);
                pos += 4;
                WireValue::I32(v)
            }
            2 => {
                let (len, n) = read_varint(&bytes[pos..])?;
                pos += n;
                let len = len as usize;
                if pos.checked_add(len).is_none_or(|end| end > bytes.len()) {
                    return None;
                }
                let sub = &bytes[pos..pos + len];
                pos += len;
                classify_len_delim(sub, depth)
            }
            // 3/4 = deprecated groups, 6/7 = invalid: reject the whole buffer.
            _ => return None,
        };
        fields.push(Field {
            number,
            wire_type,
            value,
        });
    }
    Some(fields)
}

/// Disambiguate a length-delimited payload into a nested message, string, or
/// raw bytes. Biases toward `Str` for plain printable text: a payload that is
/// clean UTF-8 (no control characters) is treated as text even if it *also*
/// happens to parse as a sub-message (short printable ASCII runs frequently do).
fn classify_len_delim(sub: &[u8], depth: usize) -> WireValue {
    let text: Option<String> = std::str::from_utf8(sub)
        .ok()
        .filter(|s| !s.is_empty() && !s.chars().any(|c| c.is_control()))
        .map(|s| s.to_string());

    if text.is_none()
        && let Some(fields) = decode_message_inner(sub, depth + 1)
        && !fields.is_empty()
    {
        return WireValue::Message(fields);
    }

    match text {
        Some(s) => WireValue::Str(s),
        None if sub.is_empty() => WireValue::Str(String::new()),
        None => WireValue::Bytes(sub.to_vec()),
    }
}

/// Read a base-128 varint. Returns `(value, bytes_consumed)`, or `None` if the
/// buffer ends mid-varint or the varint is over-long (> 10 bytes).
fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        if i >= 10 {
            return None; // over-long varint
        }
        let b = bytes[i];
        result |= ((b & 0x7f) as u64) << shift;
        i += 1;
        if b & 0x80 == 0 {
            return Some((result, i));
        }
        shift += 7;
    }
    None
}

/// A loaded set of `.proto` descriptors for named decoding.
#[cfg(feature = "proto")]
pub struct ProtoPool {
    pool: prost_reflect::DescriptorPool,
}

#[cfg(feature = "proto")]
impl ProtoPool {
    /// Recursively compile every `*.proto` under `dir` into a descriptor pool.
    pub fn load_dir(dir: &std::path::Path) -> Result<Self, crate::error::CharlesError> {
        use crate::error::CharlesError;

        let mut files = Vec::new();
        collect_protos(dir, &mut files)?;

        let mut compiler = protox::Compiler::new([dir])
            .map_err(|e| CharlesError::Parse(format!("protox init: {e}")))?;
        compiler
            .open_files(files)
            .map_err(|e| CharlesError::Parse(format!("protox compile: {e}")))?;
        let bytes = compiler.encode_file_descriptor_set();
        let pool = prost_reflect::DescriptorPool::decode(bytes.as_slice())
            .map_err(|e| CharlesError::Parse(format!("descriptor pool: {e}")))?;
        Ok(ProtoPool { pool })
    }

    /// Decode `bytes` against the message type `fq_name`, returning pretty JSON.
    /// `None` if the type is unknown or the bytes do not decode.
    pub fn decode_named(&self, fq_name: &str, bytes: &[u8]) -> Option<String> {
        let desc = self.pool.get_message_by_name(fq_name)?;
        let msg = prost_reflect::DynamicMessage::decode(desc, bytes).ok()?;
        let mut buf = Vec::new();
        let opts = prost_reflect::SerializeOptions::new()
            .use_proto_field_name(true)
            .skip_default_fields(false);
        msg.serialize_with_options(&mut serde_json::Serializer::pretty(&mut buf), &opts)
            .ok()?;
        String::from_utf8(buf).ok()
    }
}

/// Recursively collect `*.proto` files under `dir`.
#[cfg(feature = "proto")]
fn collect_protos(
    dir: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<(), crate::error::CharlesError> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_protos(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("proto") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_and_string() {
        // field 1 = varint 150 (08 96 01), field 2 = "abc" (12 03 61 62 63)
        let bytes = [0x08, 0x96, 0x01, 0x12, 0x03, 0x61, 0x62, 0x63];
        let tree = try_decode_to_tree(&bytes).expect("decodes");
        assert!(tree.contains("1: 150"), "tree:\n{tree}");
        assert!(tree.contains("2: \"abc\""), "tree:\n{tree}");
    }

    #[test]
    fn nested_message() {
        // field 3 = message { field 1 = varint 42 }  (1a 02 08 2a)
        let bytes = [0x1a, 0x02, 0x08, 0x2a];
        let tree = try_decode_to_tree(&bytes).expect("decodes");
        assert!(tree.contains("3 {"), "tree:\n{tree}");
        assert!(tree.contains("1: 42"), "tree:\n{tree}");
    }

    #[test]
    fn malformed_dangling_varint() {
        // Tag claims a varint field but the value bytes are missing.
        assert!(decode_message(&[0x08]).is_none());
    }

    // The real-captured-body test lives in tests/real_capture.rs (gitignored)
    // to keep real traffic out of git.

    #[cfg(feature = "proto")]
    #[test]
    fn named_decode_ping() {
        use crate::session::body::DecodeOptions;
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let proto_path = dir.path().join("ping.proto");
        let mut f = std::fs::File::create(&proto_path).unwrap();
        write!(
            f,
            "syntax = \"proto3\";\nmessage Ping {{ string msg = 1; int32 n = 2; }}\n"
        )
        .unwrap();
        f.flush().unwrap();
        drop(f);

        let pool = ProtoPool::load_dir(dir.path()).expect("load_dir");
        // field 1 = "hello" (0a 05 68 65 6c 6c 6f), field 2 = 7 (10 07)
        let bytes = [0x0a, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x10, 0x07];
        let opts = DecodeOptions {
            pool: Some(&pool),
            proto_type: Some("Ping"),
        };
        let (json, named) = try_decode(&bytes, &opts).expect("decodes");
        assert!(named, "should be a named decode");
        assert!(json.contains("hello"), "json:\n{json}");
        assert!(json.contains('7'), "json:\n{json}");
    }
}
