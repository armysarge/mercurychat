//! Typed MCH/1 payloads. JSON payloads use explicit schemas: unknown fields
//! and duplicate keys are rejected (serde derive reports duplicates), and every
//! string is length-bounded after parsing.

use crate::{Frame, FrameType, ObjectId, ProtocolError, MAX_CHAT_BYTES, MAX_CHUNK_DATA};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

fn bad(msg: impl Into<String>) -> ProtocolError {
    ProtocolError::BadPayload(msg.into())
}

fn check_len(field: &str, s: &str, max: usize) -> Result<(), ProtocolError> {
    if s.len() > max {
        return Err(bad(format!("{field} longer than {max} bytes")));
    }
    Ok(())
}

fn to_json<T: Serialize>(v: &T, max: usize) -> Result<Vec<u8>, ProtocolError> {
    let out = serde_json::to_vec(v).map_err(|e| bad(e.to_string()))?;
    if out.len() > max {
        return Err(bad(format!("encoded JSON {} > {max}", out.len())));
    }
    Ok(out)
}

fn from_json<T: DeserializeOwned>(b: &[u8]) -> Result<T, ProtocolError> {
    let s = std::str::from_utf8(b).map_err(|_| bad("JSON payload is not UTF-8"))?;
    serde_json::from_str(s).map_err(|e| bad(e.to_string()))
}

/// HELLO (0x01): sent once by each peer after CONNECTED.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub app: String,
    pub app_version: String,
    pub proto_minor: u8,
    /// 128-bit session nonce, lowercase hex.
    pub nonce: String,
    pub callsign: String,
    pub capabilities: Vec<String>,
    pub max_frame: u32,
    pub max_file: u64,
    pub max_chunk: u32,
}

impl Hello {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        check_len("app", &self.app, 64)?;
        check_len("app_version", &self.app_version, 64)?;
        check_len("callsign", &self.callsign, 15)?;
        if ObjectId::from_hex(&self.nonce).is_none() {
            return Err(bad("nonce must be 32 hex chars"));
        }
        if self.capabilities.len() > 32 || self.capabilities.iter().any(|c| c.len() > 32) {
            return Err(bad("capabilities too long"));
        }
        if self.max_frame < crate::MIN_PEER_FRAME {
            return Err(bad(format!("peer max_frame {} below MCH/1 minimum", self.max_frame)));
        }
        if self.max_chunk == 0 {
            return Err(bad("max_chunk must be > 0"));
        }
        Ok(())
    }
    pub fn to_frame(&self) -> Result<Frame, ProtocolError> {
        self.validate()?;
        Ok(Frame::new(FrameType::Hello, ObjectId::ZERO, to_json(self, 2048)?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let h: Hello = from_json(&f.payload)?;
        h.validate()?;
        Ok(h)
    }
}

/// HELLO_ACK (0x02): echoes the remote nonce with the agreed version and capabilities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelloAck {
    pub nonce: String,
    pub major: u8,
    pub minor: u8,
    pub capabilities: Vec<String>,
}

impl HelloAck {
    pub fn to_frame(&self) -> Result<Frame, ProtocolError> {
        Ok(Frame::new(FrameType::HelloAck, ObjectId::ZERO, to_json(self, 1024)?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let a: HelloAck = from_json(&f.payload)?;
        if a.capabilities.len() > 32 {
            return Err(bad("capabilities too long"));
        }
        Ok(a)
    }
}

/// CHAT (0x10): UTF-8 text, newlines preserved, <= 4096 bytes.
pub fn chat_frame(id: ObjectId, text: &str) -> Result<Frame, ProtocolError> {
    if text.len() > MAX_CHAT_BYTES {
        return Err(bad(format!("chat {} bytes > {MAX_CHAT_BYTES}", text.len())));
    }
    Ok(Frame::new(FrameType::Chat, id, text.as_bytes().to_vec()))
}

pub fn chat_text(f: &Frame) -> Result<String, ProtocolError> {
    String::from_utf8(f.payload.clone()).map_err(|_| bad("chat is not valid UTF-8"))
}

/// FILE_OFFER (0x20).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOffer {
    pub name: String,
    pub size: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

impl FileOffer {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        check_len("name", &self.name, 255)?;
        if self.name.is_empty() {
            return Err(bad("empty file name"));
        }
        if self.sha256.len() != 64 || !self.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(bad("sha256 must be 64 lowercase hex chars"));
        }
        if let Some(m) = &self.mime {
            check_len("mime", m, 127)?;
        }
        Ok(())
    }
    pub fn to_frame(&self, transfer: ObjectId) -> Result<Frame, ProtocolError> {
        self.validate()?;
        Ok(Frame::new(FrameType::FileOffer, transfer, to_json(self, 2048)?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let o: FileOffer = from_json(&f.payload)?;
        o.validate()?;
        Ok(o)
    }
}

/// FILE_ACCEPT (0x21).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileAccept {
    pub chunk: u32,
}

impl FileAccept {
    pub fn to_frame(&self, transfer: ObjectId) -> Result<Frame, ProtocolError> {
        Ok(Frame::new(FrameType::FileAccept, transfer, to_json(self, 256)?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let a: FileAccept = from_json(&f.payload)?;
        if a.chunk == 0 || a.chunk as usize > MAX_CHUNK_DATA {
            return Err(bad("chunk limit out of range"));
        }
        Ok(a)
    }
}

/// Bounded reason for FILE_REJECT / FILE_CANCEL / ERROR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reason {
    pub code: String,
    #[serde(default)]
    pub text: String,
}

impl Reason {
    pub fn new(code: &str, text: &str) -> Self {
        let mut t = text.to_string();
        t.truncate(160);
        Reason { code: code.into(), text: t }
    }
    pub fn to_frame(&self, ftype: FrameType, id: ObjectId) -> Result<Frame, ProtocolError> {
        check_len("code", &self.code, 32)?;
        Ok(Frame::new(ftype, id, to_json(self, 256)?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let r: Reason = from_json(&f.payload)?;
        check_len("code", &r.code, 32)?;
        check_len("text", &r.text, 200)?;
        Ok(r)
    }
}

/// FILE_CHUNK (0x23): 8-byte offset then raw data.
pub fn chunk_frame(transfer: ObjectId, offset: u64, data: &[u8]) -> Result<Frame, ProtocolError> {
    if data.is_empty() || data.len() > MAX_CHUNK_DATA {
        return Err(bad("chunk data size out of range"));
    }
    let mut p = Vec::with_capacity(8 + data.len());
    p.extend_from_slice(&offset.to_be_bytes());
    p.extend_from_slice(data);
    Ok(Frame::new(FrameType::FileChunk, transfer, p))
}

pub fn parse_chunk(f: &Frame) -> Result<(u64, &[u8]), ProtocolError> {
    if f.payload.len() < 9 {
        return Err(bad("chunk without data"));
    }
    let off = u64::from_be_bytes(f.payload[..8].try_into().unwrap());
    Ok((off, &f.payload[8..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Hello {
        Hello {
            app: "MercuryChat".into(),
            app_version: "0.0.1".into(),
            proto_minor: 0,
            nonce: "00112233445566778899aabbccddeeff".into(),
            callsign: "N0CALL".into(),
            capabilities: vec!["chat".into(), "file".into()],
            max_frame: 8192,
            max_file: 262144,
            max_chunk: 1024,
        }
    }

    #[test]
    fn hello_roundtrip() {
        let f = hello().to_frame().unwrap();
        assert_eq!(Hello::from_frame(&f).unwrap(), hello());
    }

    #[test]
    fn duplicate_key_rejected() {
        let json = br#"{"chunk":512,"chunk":1024}"#;
        let f = Frame::new(FrameType::FileAccept, ObjectId::ZERO, json.to_vec());
        assert!(FileAccept::from_frame(&f).is_err());
    }

    #[test]
    fn unknown_field_rejected() {
        let json = br#"{"chunk":512,"extra":1}"#;
        let f = Frame::new(FrameType::FileAccept, ObjectId::ZERO, json.to_vec());
        assert!(FileAccept::from_frame(&f).is_err());
    }

    /// Over-long HELLO fields from a peer are refused on receipt.
    #[test]
    fn over_long_hello_fields_rejected() {
        let mut long_app = hello();
        long_app.app = "M".repeat(65);
        let mut long_call = hello();
        long_call.callsign = "A".repeat(16);
        for h in [long_app, long_call] {
            let f = Frame::new(FrameType::Hello, ObjectId::ZERO, serde_json::to_vec(&h).unwrap());
            assert!(Hello::from_frame(&f).is_err(), "{h:?}");
        }
        let mut ok = hello();
        ok.app = "M".repeat(64);
        assert!(Hello::from_frame(&ok.to_frame().unwrap()).is_ok());
    }

    #[test]
    fn small_peer_frame_rejected() {
        let mut h = hello();
        h.max_frame = 1024;
        assert!(h.to_frame().is_err());
    }

    #[test]
    fn invalid_utf8_chat_rejected() {
        let f = Frame::new(FrameType::Chat, ObjectId::ZERO, vec![0xF0, 0x28, 0x8C, 0x28]);
        assert!(chat_text(&f).is_err());
    }

    #[test]
    fn offer_hash_must_be_lowercase_hex() {
        let o = FileOffer { name: "a.bin".into(), size: 1, sha256: "A".repeat(64), mime: None };
        assert!(o.validate().is_err());
    }

    #[test]
    fn chunk_roundtrip() {
        let f = chunk_frame(ObjectId::ZERO, 4096, &[0xC0, 0xDB]).unwrap();
        let (o, d) = parse_chunk(&f).unwrap();
        assert_eq!((o, d), (4096, &[0xC0u8, 0xDB][..]));
    }
}
