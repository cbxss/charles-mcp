use crate::session::{RawBody, WsDirection, WsMessage, WsOpcode};

const OPCODE_CONTINUATION: u8 = 0x0;

pub fn parse_messages(bytes: &[u8], direction: WsDirection) -> Vec<WsMessage> {
    let mut messages = Vec::new();
    let mut pending: Option<(WsOpcode, Vec<u8>)> = None;
    let mut pos = 0usize;
    let total = bytes.len();

    while pos + 2 <= total {
        let b0 = bytes[pos];
        let b1 = bytes[pos + 1];
        let fin = b0 & 0x80 != 0;
        let opcode = b0 & 0x0F;
        let masked = b1 & 0x80 != 0;
        let len7 = b1 & 0x7F;

        let mut cursor = pos + 2;
        let payload_len: usize = match len7 {
            126 => {
                if cursor + 2 > total {
                    break;
                }
                let n = u16::from_be_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
                cursor += 2;
                n
            }
            127 => {
                if cursor + 8 > total {
                    break;
                }
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&bytes[cursor..cursor + 8]);
                cursor += 8;
                u64::from_be_bytes(arr) as usize
            }
            n => n as usize,
        };

        let mask_key = if masked {
            if cursor + 4 > total {
                break;
            }
            let key = [
                bytes[cursor],
                bytes[cursor + 1],
                bytes[cursor + 2],
                bytes[cursor + 3],
            ];
            cursor += 4;
            Some(key)
        } else {
            None
        };

        let data_end = match cursor.checked_add(payload_len) {
            Some(end) if end <= total => end,
            _ => break,
        };
        let mut payload = bytes[cursor..data_end].to_vec();
        if let Some(key) = mask_key {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= key[i & 3];
            }
        }
        pos = data_end;

        if opcode & 0x08 != 0 {
            messages.push(message(direction, map_opcode(opcode), payload));
        } else if opcode == OPCODE_CONTINUATION {
            match pending.as_mut() {
                Some((_, buf)) => {
                    buf.extend_from_slice(&payload);
                    if fin {
                        let (op, buf) = pending.take().expect("pending is Some");
                        messages.push(message(direction, op, buf));
                    }
                }
                None if fin => messages.push(message(direction, WsOpcode::Other(0), payload)),
                None => pending = Some((WsOpcode::Other(0), payload)),
            }
        } else {
            if let Some((op, buf)) = pending.take() {
                messages.push(message(direction, op, buf));
            }
            let op = map_opcode(opcode);
            if fin {
                messages.push(message(direction, op, payload));
            } else {
                pending = Some((op, payload));
            }
        }
    }

    messages
}

fn map_opcode(op: u8) -> WsOpcode {
    match op {
        0x1 => WsOpcode::Text,
        0x2 => WsOpcode::Binary,
        0x8 => WsOpcode::Close,
        0x9 => WsOpcode::Ping,
        0xA => WsOpcode::Pong,
        other => WsOpcode::Other(other),
    }
}

fn message(direction: WsDirection, opcode: WsOpcode, bytes: Vec<u8>) -> WsMessage {
    WsMessage {
        direction,
        opcode,
        payload: RawBody {
            bytes,
            captured: true,
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    fn b64(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .unwrap()
    }

    #[test]
    fn piesocket_single_text_message() {
        let bytes = b64(include_str!("../../tests/fixtures/ws_piesocket_recv.b64"));
        let msgs = parse_messages(&bytes, WsDirection::Received);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].opcode, WsOpcode::Text);
        let text = String::from_utf8(msgs[0].payload.bytes.clone()).unwrap();
        assert!(text.contains("Unkown API Key"), "got: {text}");
    }

    #[test]
    fn synthetic_unmasked_text() {
        let frame = [0x81u8, 0x03, b'a', b'b', b'c'];
        let msgs = parse_messages(&frame, WsDirection::Received);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].opcode, WsOpcode::Text);
        assert_eq!(msgs[0].payload.bytes, b"abc");
    }

    #[test]
    fn synthetic_masked_text_unmasks() {
        let key = [0x12u8, 0x34, 0x56, 0x78];
        let plaintext = b"hello world";
        let mut frame = vec![0x81u8, 0x80 | plaintext.len() as u8];
        frame.extend_from_slice(&key);
        for (i, byte) in plaintext.iter().enumerate() {
            frame.push(byte ^ key[i % 4]);
        }
        let msgs = parse_messages(&frame, WsDirection::Sent);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].opcode, WsOpcode::Text);
        assert_eq!(msgs[0].payload.bytes, plaintext);
    }

    #[test]
    fn synthetic_fragmented_text_reassembles() {
        let frame = [0x01u8, 0x02, b'a', b'b', 0x80, 0x01, b'c'];
        let msgs = parse_messages(&frame, WsDirection::Received);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].opcode, WsOpcode::Text);
        assert_eq!(msgs[0].payload.bytes, b"abc");
    }

    #[test]
    fn control_frame_emitted_standalone() {
        let frame = [0x89u8, 0x00, 0x81, 0x02, b'h', b'i'];
        let msgs = parse_messages(&frame, WsDirection::Received);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].opcode, WsOpcode::Ping);
        assert!(msgs[0].payload.bytes.is_empty());
        assert_eq!(msgs[1].opcode, WsOpcode::Text);
        assert_eq!(msgs[1].payload.bytes, b"hi");
    }

    #[test]
    fn empty_stream_yields_nothing() {
        assert!(parse_messages(&[], WsDirection::Received).is_empty());
    }

    #[test]
    fn truncated_trailing_frame_is_dropped() {
        let frame = [0x81u8, 0x02, b'a', b'b', 0x81, 0x05, b'x', b'y'];
        let msgs = parse_messages(&frame, WsDirection::Received);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].payload.bytes, b"ab");
    }
}
