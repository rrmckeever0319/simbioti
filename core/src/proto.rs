use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

pub const KIND_CONTROL: u8 = 1;
pub const KIND_CHUNK: u8 = 2;

/// Raw file bytes per chunk. The encoded frame stays under [`crate::crypto::MAX_PLAINTEXT`].
pub const CHUNK_DATA: usize = 48 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Control {
    Have {
        hash: String,
        size: u64,
        rel_path: String,
    },
    HaveEnd,
    Need {
        hash: String,
    },
    NeedEnd,
    FileStart {
        hash: String,
        size: u64,
        rel_path: String,
    },
    FileEnd {
        hash: String,
    },
    Done,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Control(Control),
    Chunk {
        hash: [u8; 32],
        offset: u64,
        data: Vec<u8>,
    },
}

pub fn encode(message: &Message) -> Result<Vec<u8>> {
    match message {
        Message::Control(control) => {
            let mut out = vec![KIND_CONTROL];
            serde_json::to_writer(&mut out, control)
                .map_err(|err| Error::Protocol(err.to_string()))?;
            Ok(out)
        }
        Message::Chunk { hash, offset, data } => {
            if data.len() > CHUNK_DATA {
                return Err(Error::Protocol(format!(
                    "chunk of {} bytes exceeds {CHUNK_DATA}",
                    data.len()
                )));
            }
            let mut out = Vec::with_capacity(1 + 32 + 8 + data.len());
            out.push(KIND_CHUNK);
            out.extend_from_slice(hash);
            out.extend_from_slice(&offset.to_be_bytes());
            out.extend_from_slice(data);
            Ok(out)
        }
    }
}

pub fn decode(buf: &[u8]) -> Result<Message> {
    if buf.is_empty() {
        return Err(Error::Protocol("empty frame".into()));
    }
    match buf[0] {
        KIND_CONTROL => {
            let control = serde_json::from_slice(&buf[1..])
                .map_err(|err| Error::Protocol(err.to_string()))?;
            Ok(Message::Control(control))
        }
        KIND_CHUNK => {
            if buf.len() < 1 + 32 + 8 {
                return Err(Error::Protocol("short chunk".into()));
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&buf[1..33]);
            let mut offset_bytes = [0u8; 8];
            offset_bytes.copy_from_slice(&buf[33..41]);
            let offset = u64::from_be_bytes(offset_bytes);
            Ok(Message::Chunk {
                hash,
                offset,
                data: buf[41..].to_vec(),
            })
        }
        kind => Err(Error::Protocol(format!("unknown message kind {kind}"))),
    }
}

pub fn parse_hash(text: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(text.trim()).map_err(|err| Error::Protocol(format!("hash: {err}")))?;
    if bytes.len() != 32 {
        return Err(Error::Protocol(format!(
            "hash must be 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

pub fn norm_hash(text: &str) -> Result<String> {
    Ok(hex::encode(parse_hash(text)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_and_chunk_round_trip() {
        let control = Message::Control(Control::Have {
            hash: "ab".repeat(32),
            size: 3,
            rel_path: "album/pic.jpg".into(),
        });
        assert_eq!(decode(&encode(&control).unwrap()).unwrap(), control);

        let mut hash = [0u8; 32];
        hash[0] = 4;
        let chunk = Message::Chunk {
            hash,
            offset: 48 * 1024,
            data: vec![9, 8, 7],
        };
        assert_eq!(decode(&encode(&chunk).unwrap()).unwrap(), chunk);
    }

    #[test]
    fn rejects_unknown_kind_and_bad_hash() {
        assert!(decode(&[7, b'{', b'}']).is_err());
        assert!(parse_hash("abcd").is_err());
    }
}
