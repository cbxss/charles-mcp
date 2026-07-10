use crate::session::body::DecodeOptions;

const MAX_DEPTH: usize = 100;

#[derive(Debug, Clone, PartialEq)]
pub enum WireValue {
    Varint(u64),
    I64(u64),
    I32(u32),
    Message(Vec<Field>),
    Str(String),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub number: u32,
    pub wire_type: u8,
    pub value: WireValue,
}

pub fn try_decode(bytes: &[u8], opts: &DecodeOptions) -> Option<(String, bool)> {
    #[cfg(feature = "proto")]
    if let Some(pool) = opts.pool
        && let Ok(json) = pool.decode_named(opts.proto_type, bytes)
    {
        return Some((json, true));
    }
    #[cfg(not(feature = "proto"))]
    let _ = opts;

    let tree = try_decode_to_tree(bytes)?;
    Some((tree, false))
}

pub fn decode_message(bytes: &[u8]) -> Option<Vec<Field>> {
    decode_message_inner(bytes, 0)
}

pub fn try_decode_to_tree(bytes: &[u8]) -> Option<String> {
    let fields = decode_message(bytes)?;
    if fields.is_empty() {
        return None;
    }
    let mut out = String::new();
    render_tree(&mut out, &fields, 0);
    Some(out)
}

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

fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    let mut i = 0;
    while i < bytes.len() {
        if i >= 10 {
            return None;
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

#[cfg(feature = "proto")]
#[derive(Debug, Clone, PartialEq)]
pub enum NamedError {
    NoType { candidates: Vec<String> },
    UnknownType { name: String, candidates: Vec<String> },
    Ambiguous { name: String, candidates: Vec<String> },
    DecodeFailed { name: String, msg: String },
}

#[cfg(feature = "proto")]
impl std::fmt::Display for NamedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NamedError::NoType { candidates } => write!(
                f,
                "no proto_type given and the file defines {} messages; pass one of: {}",
                candidates.len(),
                candidates.join(", ")
            ),
            NamedError::UnknownType { name, candidates } => write!(
                f,
                "unknown proto_type '{name}'; the file defines: {}",
                candidates.join(", ")
            ),
            NamedError::Ambiguous { name, candidates } => write!(
                f,
                "proto_type '{name}' is ambiguous; qualify with package: {}",
                candidates.join(", ")
            ),
            NamedError::DecodeFailed { name, msg } => {
                write!(f, "'{name}' did not match the wire bytes: {msg}")
            }
        }
    }
}

#[cfg(feature = "proto")]
pub struct ProtoPool {
    pool: prost_reflect::DescriptorPool,
    primary: String,
}

#[cfg(feature = "proto")]
impl ProtoPool {
    pub fn load_file(
        file: &std::path::Path,
        root: Option<&std::path::Path>,
    ) -> Result<Self, crate::error::CharlesError> {
        use crate::error::CharlesError;

        let root_buf = match root {
            Some(r) => r.to_path_buf(),
            None => file
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
        };

        let mut compiler = protox::Compiler::new([root_buf.as_path()])
            .map_err(|e| CharlesError::Parse(format!("protox init: {e}")))?;
        compiler.include_imports(true);
        compiler.open_files([file.to_path_buf()]).map_err(|e| {
            CharlesError::Parse(format!("protox compile {}: {e}", file.display()))
        })?;
        let bytes = compiler.encode_file_descriptor_set();
        let pool = prost_reflect::DescriptorPool::decode(bytes.as_slice())
            .map_err(|e| CharlesError::Parse(format!("descriptor pool: {e}")))?;
        let primary = file
            .strip_prefix(&root_buf)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        Ok(ProtoPool { pool, primary })
    }

    fn primary_messages(&self) -> Vec<prost_reflect::MessageDescriptor> {
        self.pool
            .all_messages()
            .filter(|m| m.parent_file().name() == self.primary)
            .collect()
    }

    fn resolve(
        &self,
        proto_type: Option<&str>,
    ) -> Result<prost_reflect::MessageDescriptor, NamedError> {
        let candidates = self.primary_messages();
        match proto_type {
            None => {
                let mut it = candidates.into_iter();
                match (it.next(), it.next()) {
                    (Some(only), None) => Ok(only),
                    (first, second) => Err(NamedError::NoType {
                        candidates: names(first.into_iter().chain(second).chain(it)),
                    }),
                }
            }
            Some(name) => {
                if let Some(m) = self.pool.get_message_by_name(name) {
                    return Ok(m);
                }
                let hits: Vec<_> = candidates
                    .iter()
                    .filter(|m| m.name() == name || m.full_name() == name)
                    .cloned()
                    .collect();
                match hits.len() {
                    1 => Ok(hits.into_iter().next().unwrap()),
                    0 => Err(NamedError::UnknownType {
                        name: name.to_string(),
                        candidates: names(candidates.into_iter()),
                    }),
                    _ => Err(NamedError::Ambiguous {
                        name: name.to_string(),
                        candidates: names(hits.into_iter()),
                    }),
                }
            }
        }
    }

    pub fn resolve_name(&self, proto_type: Option<&str>) -> Result<String, NamedError> {
        self.resolve(proto_type).map(|m| m.full_name().to_string())
    }

    pub fn decode_named(&self, proto_type: Option<&str>, bytes: &[u8]) -> Result<String, NamedError> {
        let desc = self.resolve(proto_type)?;
        let name = desc.full_name().to_string();
        let msg = prost_reflect::DynamicMessage::decode(desc, bytes).map_err(|e| {
            NamedError::DecodeFailed {
                name: name.clone(),
                msg: e.to_string(),
            }
        })?;
        let mut buf = Vec::new();
        let opts = prost_reflect::SerializeOptions::new()
            .use_proto_field_name(true)
            .skip_default_fields(false);
        msg.serialize_with_options(&mut serde_json::Serializer::pretty(&mut buf), &opts)
            .map_err(|e| NamedError::DecodeFailed {
                name: name.clone(),
                msg: e.to_string(),
            })?;
        String::from_utf8(buf).map_err(|e| NamedError::DecodeFailed { name, msg: e.to_string() })
    }
}

#[cfg(feature = "proto")]
fn names(msgs: impl Iterator<Item = prost_reflect::MessageDescriptor>) -> Vec<String> {
    let mut out: Vec<String> = msgs.map(|m| m.full_name().to_string()).collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_and_string() {
        let bytes = [0x08, 0x96, 0x01, 0x12, 0x03, 0x61, 0x62, 0x63];
        let tree = try_decode_to_tree(&bytes).expect("decodes");
        assert!(tree.contains("1: 150"), "tree:\n{tree}");
        assert!(tree.contains("2: \"abc\""), "tree:\n{tree}");
    }

    #[test]
    fn nested_message() {
        let bytes = [0x1a, 0x02, 0x08, 0x2a];
        let tree = try_decode_to_tree(&bytes).expect("decodes");
        assert!(tree.contains("3 {"), "tree:\n{tree}");
        assert!(tree.contains("1: 42"), "tree:\n{tree}");
    }

    #[test]
    fn malformed_dangling_varint() {
        assert!(decode_message(&[0x08]).is_none());
    }

    #[cfg(feature = "proto")]
    fn write_proto(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let proto_path = dir.path().join("ping.proto");
        let mut f = std::fs::File::create(&proto_path).unwrap();
        write!(f, "{body}").unwrap();
        f.flush().unwrap();
        drop(f);
        (dir, proto_path)
    }

    #[cfg(feature = "proto")]
    #[test]
    fn named_decode_ping() {
        use crate::session::body::DecodeOptions;

        let (_dir, proto_path) =
            write_proto("syntax = \"proto3\";\nmessage Ping { string msg = 1; int32 n = 2; }\n");
        let pool = ProtoPool::load_file(&proto_path, None).expect("load_file");
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

    #[cfg(feature = "proto")]
    #[test]
    fn short_name_resolves_within_package() {
        let (_dir, proto_path) = write_proto(
            "syntax = \"proto3\";\npackage foo.bar;\nmessage Ping { string msg = 1; }\n",
        );
        let pool = ProtoPool::load_file(&proto_path, None).expect("load_file");
        let bytes = [0x0a, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f];
        let json = pool.decode_named(Some("Ping"), &bytes).expect("short name");
        assert!(json.contains("hello"), "json:\n{json}");
        let json = pool
            .decode_named(Some("foo.bar.Ping"), &bytes)
            .expect("fq name");
        assert!(json.contains("hello"), "json:\n{json}");
    }

    #[cfg(feature = "proto")]
    #[test]
    fn single_message_infers_type() {
        let (_dir, proto_path) =
            write_proto("syntax = \"proto3\";\nmessage Ping { string msg = 1; }\n");
        let pool = ProtoPool::load_file(&proto_path, None).expect("load_file");
        let bytes = [0x0a, 0x05, 0x68, 0x65, 0x6c, 0x6c, 0x6f];
        let json = pool.decode_named(None, &bytes).expect("inferred");
        assert!(json.contains("hello"), "json:\n{json}");
    }

    #[cfg(feature = "proto")]
    #[test]
    fn ambiguous_short_name_is_reported() {
        let (_dir, proto_path) = write_proto(
            "syntax = \"proto3\";\npackage demo;\n\
             message A { message Ping { string x = 1; } }\n\
             message B { message Ping { int32 y = 1; } }\n",
        );
        let pool = ProtoPool::load_file(&proto_path, None).expect("load_file");
        let err = pool.decode_named(Some("Ping"), &[0x0a]).unwrap_err();
        match err {
            NamedError::Ambiguous { candidates, .. } => {
                assert!(
                    candidates.iter().any(|c| c == "demo.A.Ping")
                        && candidates.iter().any(|c| c == "demo.B.Ping"),
                    "got: {candidates:?}"
                );
            }
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[cfg(feature = "proto")]
    #[test]
    fn unknown_type_lists_candidates() {
        let (_dir, proto_path) = write_proto(
            "syntax = \"proto3\";\nmessage Ping { string msg = 1; }\nmessage Pong { int32 n = 1; }\n",
        );
        let pool = ProtoPool::load_file(&proto_path, None).expect("load_file");
        let err = pool.decode_named(Some("Nope"), &[0x0a]).unwrap_err();
        match err {
            NamedError::UnknownType { candidates, .. } => {
                assert!(candidates.iter().any(|c| c == "Ping"), "got: {candidates:?}");
                assert!(candidates.iter().any(|c| c == "Pong"), "got: {candidates:?}");
            }
            other => panic!("expected UnknownType, got {other:?}"),
        }
        let err = pool.decode_named(None, &[0x0a]).unwrap_err();
        assert!(matches!(err, NamedError::NoType { .. }), "got: {err:?}");
    }
}
