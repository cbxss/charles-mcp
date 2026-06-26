//! Detect the on-disk/in-memory session format and dispatch to a parser.

use std::io::Read;

use super::Transaction;
use crate::error::CharlesError;

#[derive(Debug, PartialEq, Eq)]
pub enum Format {
    Har,
    Chlsj,
    Xml,
    Native,
    Gzip,
}

/// Best-effort content sniff (ignores leading whitespace).
pub fn sniff(bytes: &[u8]) -> Format {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        return Format::Gzip;
    }
    let trimmed = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map(|i| &bytes[i..])
        .unwrap_or(bytes);
    match trimmed.first() {
        Some(b'[') => Format::Chlsj,
        Some(b'{') => Format::Har,
        Some(b'<') => Format::Xml,
        // Charles native `.chls` is a binary/compressed container.
        _ => Format::Native,
    }
}

/// Parse raw session bytes into transactions, transparently inflating gzip.
/// Native `.chls` cannot be parsed here — it must be converted first.
pub fn parse_bytes(bytes: Vec<u8>) -> Result<Vec<Transaction>, CharlesError> {
    match sniff(&bytes) {
        Format::Gzip => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&bytes[..]).read_to_end(&mut out)?;
            parse_bytes(out)
        }
        Format::Chlsj => super::chlsj::parse(&bytes),
        Format::Har => super::har::parse(&bytes),
        Format::Xml => Err(CharlesError::Parse(
            "XML session export is not supported; re-export as .chlsj or .har".into(),
        )),
        Format::Native => Err(CharlesError::UnknownFormat),
    }
}
