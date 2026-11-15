//! MCH/1 — MercuryChat connected-session framing (draft, docs/03 §5).
//!
//! Every frame on the ARQ data stream is a 28-byte header followed by a
//! type-specific payload. All integers are unsigned network byte order.
//!
//! | Offset | Len | Meaning                         |
//! |--------|-----|---------------------------------|
//! | 0      | 4   | ASCII `MCH1`                    |
//! | 4      | 1   | Major version (1)               |
//! | 5      | 1   | Frame type                      |
//! | 6      | 2   | Flags (all zero in v1)          |
//! | 8      | 4   | Payload length (<= 8192)        |
//! | 12     | 16  | Object ID (random 128-bit)      |
//!
//! The [`Decoder`] is incremental and bounded: it never allocates a payload
//! larger than the per-type limit, and after any error it is poisoned — the
//! caller must close the application session (no resync by magic search).

pub mod beacon;
pub mod payload;

use std::fmt;

pub const MAGIC: [u8; 4] = *b"MCH1";
pub const MAJOR: u8 = 1;
/// Highest minor version this implementation speaks.
pub const MINOR: u8 = 0;
pub const HEADER_LEN: usize = 28;
/// Absolute payload ceiling before any allocation.
pub const MAX_PAYLOAD: usize = 8192;
/// Minimum frame payload a peer must support in MCH/1.
pub const MIN_PEER_FRAME: u32 = 4096;
pub const MAX_CHAT_BYTES: usize = 4096;
pub const MAX_CHUNK_DATA: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FrameType {
    Hello = 0x01,
    HelloAck = 0x02,
    Chat = 0x10,
    ChatReceipt = 0x11,
    FileOffer = 0x20,
    FileAccept = 0x21,
    FileReject = 0x22,
    FileChunk = 0x23,
    FileEnd = 0x24,
    FileReceipt = 0x25,
    FileCancel = 0x26,
    Error = 0x7F,
}

impl FrameType {
    pub fn from_u8(v: u8) -> Option<Self> {
        use FrameType::*;
        Some(match v {
            0x01 => Hello,
            0x02 => HelloAck,
            0x10 => Chat,
            0x11 => ChatReceipt,
            0x20 => FileOffer,
            0x21 => FileAccept,
            0x22 => FileReject,
            0x23 => FileChunk,
            0x24 => FileEnd,
            0x25 => FileReceipt,
            0x26 => FileCancel,
            0x7F => Error,
            _ => return None,
        })
    }

    /// Per-type payload ceiling (docs/03 §5 types table).
    pub fn max_payload(self) -> usize {
        use FrameType::*;
        match self {
            Hello => 2048,
            HelloAck => 1024,
            Chat => MAX_CHAT_BYTES,
            ChatReceipt | FileEnd | FileReceipt => 0,
            FileOffer => 2048,
            FileAccept => 256,
            FileReject | FileCancel | Error => 256,
            FileChunk => 8 + MAX_CHUNK_DATA,
        }
    }
}

/// 128-bit object identifier (message ID, transfer ID, ...).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ObjectId(pub [u8; 16]);

impl ObjectId {
    pub const ZERO: ObjectId = ObjectId([0; 16]);
    pub fn random() -> Self {
        ObjectId(rand::random())
    }
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    pub fn from_hex(s: &str) -> Option<Self> {
        let v = hex::decode(s).ok()?;
        Some(ObjectId(v.try_into().ok()?))
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.to_hex())
    }
}
impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub ftype: FrameType,
    pub id: ObjectId,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(ftype: FrameType, id: ObjectId, payload: Vec<u8>) -> Self {
        Frame { ftype, id, payload }
    }
    pub fn empty(ftype: FrameType, id: ObjectId) -> Self {
        Frame { ftype, id, payload: Vec::new() }
    }
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone)]
pub enum ProtocolError {
    #[error("bad magic {0:02x?}")]
    BadMagic([u8; 4]),
    #[error("unsupported major version {0}")]
    BadMajor(u8),
    #[error("unknown frame type 0x{0:02x}")]
    UnknownType(u8),
    #[error("non-zero flags 0x{0:04x}")]
    BadFlags(u16),
    #[error("payload length {len} exceeds limit {limit} for {ftype:?}")]
    TooLong { ftype: FrameType, len: usize, limit: usize },
    #[error("malformed payload: {0}")]
    BadPayload(String),
    #[error("decoder closed after an earlier error")]
    Poisoned,
}

/// Serialise a frame. Rejects payloads larger than the type limit.
pub fn encode(frame: &Frame) -> Result<Vec<u8>, ProtocolError> {
    let limit = frame.ftype.max_payload().min(MAX_PAYLOAD);
    if frame.payload.len() > limit {
        return Err(ProtocolError::TooLong { ftype: frame.ftype, len: frame.payload.len(), limit });
    }
    let mut out = Vec::with_capacity(frame.encoded_len());
    out.extend_from_slice(&MAGIC);
    out.push(MAJOR);
    out.push(frame.ftype as u8);
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(frame.payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&frame.id.0);
    out.extend_from_slice(&frame.payload);
    Ok(out)
}

/// Incremental, bounded MCH/1 stream decoder.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
    poisoned: bool,
    /// Negotiated FILE_CHUNK data limit (<= MAX_CHUNK_DATA).
    chunk_limit: Option<usize>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a negotiated chunk-data limit (bytes of file data per FILE_CHUNK).
    pub fn set_chunk_limit(&mut self, limit: usize) {
        self.chunk_limit = Some(limit.min(MAX_CHUNK_DATA));
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Bytes buffered but not yet forming a complete frame.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if !self.poisoned {
            self.buf.extend_from_slice(bytes);
        }
    }

    /// Return the next complete frame, `Ok(None)` if more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, ProtocolError> {
        if self.poisoned {
            return Err(ProtocolError::Poisoned);
        }
        match self.try_next() {
            Err(e) => {
                self.poisoned = true;
                self.buf.clear();
                Err(e)
            }
            ok => ok,
        }
    }

    fn try_next(&mut self) -> Result<Option<Frame>, ProtocolError> {
        // Validate header fields as soon as they are available, so a bad
        // stream is rejected without waiting for a bogus length to fill.
        let b = &self.buf;
        let have = b.len();
        let magic_n = have.min(4);
        if b[..magic_n] != MAGIC[..magic_n] {
            let mut m = [0u8; 4];
            m[..magic_n].copy_from_slice(&b[..magic_n]);
            return Err(ProtocolError::BadMagic(m));
        }
        if have > 4 && b[4] != MAJOR {
            return Err(ProtocolError::BadMajor(b[4]));
        }
        let ftype = if have > 5 {
            Some(FrameType::from_u8(b[5]).ok_or(ProtocolError::UnknownType(b[5]))?)
        } else {
            None
        };
        if have >= 8 {
            let flags = u16::from_be_bytes([b[6], b[7]]);
            if flags != 0 {
                return Err(ProtocolError::BadFlags(flags));
            }
        }
        if have < 12 {
            return Ok(None);
        }
        let ftype = ftype.expect("type present when have >= 12");
        let len = u32::from_be_bytes([b[8], b[9], b[10], b[11]]) as usize;
        let mut limit = ftype.max_payload().min(MAX_PAYLOAD);
        if ftype == FrameType::FileChunk {
            if let Some(cl) = self.chunk_limit {
                limit = 8 + cl;
            }
        }
        if len > limit {
            return Err(ProtocolError::TooLong { ftype, len, limit });
        }
        if have < HEADER_LEN + len {
            return Ok(None);
        }
        let mut id = [0u8; 16];
        id.copy_from_slice(&b[12..28]);
        let payload = b[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some(Frame { ftype, id: ObjectId(id), payload }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> ObjectId {
        ObjectId([n; 16])
    }

    #[test]
    fn roundtrip_every_split_boundary() {
        let frames = vec![
            Frame::new(FrameType::Chat, id(1), "hello\nworld — ✓".as_bytes().to_vec()),
            Frame::empty(FrameType::ChatReceipt, id(1)),
            Frame::new(FrameType::FileChunk, id(2), {
                let mut p = 0u64.to_be_bytes().to_vec();
                p.extend_from_slice(&[0xC0, 0xDB, 0xDC, 0xDD, 0x00, 0xFF]);
                p
            }),
        ];
        let mut stream = Vec::new();
        for f in &frames {
            stream.extend(encode(f).unwrap());
        }
        for split in 0..=stream.len() {
            let mut d = Decoder::new();
            let mut got = Vec::new();
            d.push(&stream[..split]);
            while let Some(f) = d.next_frame().unwrap() {
                got.push(f);
            }
            d.push(&stream[split..]);
            while let Some(f) = d.next_frame().unwrap() {
                got.push(f);
            }
            assert_eq!(got, frames, "split at {split}");
            assert_eq!(d.pending(), 0);
        }
    }

    #[test]
    fn byte_at_a_time() {
        let f = Frame::new(FrameType::Chat, id(9), vec![b'x'; MAX_CHAT_BYTES]);
        let bytes = encode(&f).unwrap();
        let mut d = Decoder::new();
        let mut out = None;
        for b in &bytes {
            d.push(std::slice::from_ref(b));
            if let Some(fr) = d.next_frame().unwrap() {
                out = Some(fr);
            }
        }
        assert_eq!(out.unwrap(), f);
    }

    #[test]
    fn rejects_bad_magic_early_and_poisons() {
        let mut d = Decoder::new();
        d.push(b"MCX");
        assert!(matches!(d.next_frame(), Err(ProtocolError::BadMagic(_))));
        d.push(b"MCH1");
        assert_eq!(d.next_frame(), Err(ProtocolError::Poisoned));
    }

    #[test]
    fn rejects_plain_text_without_waiting() {
        let mut d = Decoder::new();
        d.push(b"hi");
        assert!(matches!(d.next_frame(), Err(ProtocolError::BadMagic(_))));
    }

    #[test]
    fn rejects_bad_major_type_flags() {
        let mut h = encode(&Frame::empty(FrameType::FileEnd, id(0))).unwrap();
        h[4] = 2;
        let mut d = Decoder::new();
        d.push(&h[..5]);
        assert_eq!(d.next_frame(), Err(ProtocolError::BadMajor(2)));

        let mut h = encode(&Frame::empty(FrameType::FileEnd, id(0))).unwrap();
        h[5] = 0x55;
        let mut d = Decoder::new();
        d.push(&h);
        assert_eq!(d.next_frame(), Err(ProtocolError::UnknownType(0x55)));

        let mut h = encode(&Frame::empty(FrameType::FileEnd, id(0))).unwrap();
        h[7] = 1;
        let mut d = Decoder::new();
        d.push(&h);
        assert_eq!(d.next_frame(), Err(ProtocolError::BadFlags(1)));
    }

    #[test]
    fn rejects_huge_length_before_allocation() {
        let mut h = encode(&Frame::empty(FrameType::Chat, id(0))).unwrap();
        h[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut d = Decoder::new();
        d.push(&h[..12]);
        assert!(matches!(d.next_frame(), Err(ProtocolError::TooLong { .. })));
    }

    #[test]
    fn receipt_with_payload_rejected() {
        let mut h = encode(&Frame::empty(FrameType::ChatReceipt, id(0))).unwrap();
        h[11] = 1;
        h.push(0);
        let mut d = Decoder::new();
        d.push(&h);
        assert!(matches!(d.next_frame(), Err(ProtocolError::TooLong { .. })));
    }

    #[test]
    fn encode_refuses_oversize_chat() {
        let f = Frame::new(FrameType::Chat, id(0), vec![b'a'; MAX_CHAT_BYTES + 1]);
        assert!(encode(&f).is_err());
    }

    #[test]
    fn negotiated_chunk_limit_enforced() {
        let mut p = 0u64.to_be_bytes().to_vec();
        p.extend(vec![0u8; 600]);
        let bytes = encode(&Frame::new(FrameType::FileChunk, id(3), p)).unwrap();
        let mut d = Decoder::new();
        d.set_chunk_limit(512);
        d.push(&bytes);
        assert!(matches!(d.next_frame(), Err(ProtocolError::TooLong { .. })));
    }
}
