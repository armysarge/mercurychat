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

/// How a file's bytes travel (minor 2, M4 C1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Zstd,
}

/// FILE_OFFER (0x20). `size` and `sha256` are the file as it lands on disk;
/// `wire_size` is what the chunks carry (minor 2, M4 C1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOffer {
    pub name: String,
    pub size: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<Encoding>,
    pub wire_size: u64,
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
        if self.wire_size == 0 {
            return Err(bad("wire_size must be at least 1"));
        }
        if self.encoding.is_none() && self.wire_size != self.size {
            return Err(bad("without an encoding, wire_size equals size"));
        }
        // A file is only encoded when that makes it smaller (M4 C2).
        if self.encoding.is_some() && self.wire_size >= self.size {
            return Err(bad("an encoded file's wire_size is less than its size"));
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
        // At most 160 bytes, never splitting a character.
        let mut end = text.len().min(160);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Reason { code: code.into(), text: text[..end].to_string() }
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

// ---- Minor 1 (M3c B2): QSY and info ----

/// Highest frequency a candidate may carry: mc-core's rig limit
/// (`MAX_FREQ_HZ` in app/service/rig.rs).
pub const MAX_FREQ_HZ: u64 = 60_000_000;
/// The modes a candidate may name; empty leaves the mode as it is.
pub const QSY_MODES: [&str; 7] = ["", "USB", "LSB", "PKTUSB", "PKTLSB", "FM", "PKTFM"];
pub const MAX_CANDIDATES: usize = 5;

fn check_opt(field: &str, s: &Option<String>, max: usize) -> Result<(), ProtocolError> {
    s.as_deref().map_or(Ok(()), |s| check_len(field, s, max))
}

/// One frequency a QSY proposes: a displayed frequency (dial + offset, B4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QsyCandidate {
    pub freq_hz: u64,
    pub mode: String,
    /// 0 = the rig's default passband.
    pub passband_hz: u32,
    pub name: String,
}

impl QsyCandidate {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !(1..=MAX_FREQ_HZ).contains(&self.freq_hz) {
            return Err(bad(format!("frequency {} Hz out of range", self.freq_hz)));
        }
        if !QSY_MODES.contains(&self.mode.as_str()) {
            return Err(bad(format!("mode {:?} not allowed", self.mode)));
        }
        check_len("name", &self.name, 32)
    }
}

/// QSY_PROPOSE (0x30); the frame id is the proposal id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QsyPropose {
    pub candidates: Vec<QsyCandidate>,
    pub note: Option<String>,
}

impl QsyPropose {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if !(1..=MAX_CANDIDATES).contains(&self.candidates.len()) {
            return Err(bad(format!("{} candidates; 1 to {MAX_CANDIDATES} allowed", self.candidates.len())));
        }
        self.candidates.iter().try_for_each(QsyCandidate::validate)?;
        check_opt("note", &self.note, 64)
    }
    pub fn to_frame(&self, proposal: ObjectId) -> Result<Frame, ProtocolError> {
        self.validate()?;
        Ok(Frame::new(FrameType::QsyPropose, proposal, to_json(self, FrameType::QsyPropose.max_payload())?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let p: QsyPropose = from_json(&f.payload)?;
        p.validate()?;
        Ok(p)
    }
}

/// QSY_ANSWER (0x31); the frame id is the proposal id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "answer", rename_all = "snake_case", deny_unknown_fields)]
pub enum QsyAnswer {
    Accept { index: u8 },
    Decline { reason: Option<String> },
}

impl QsyAnswer {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            QsyAnswer::Accept { index } if *index as usize >= MAX_CANDIDATES => {
                Err(bad(format!("candidate index {index} out of range")))
            }
            QsyAnswer::Accept { .. } => Ok(()),
            QsyAnswer::Decline { reason } => check_opt("reason", reason, 32),
        }
    }
    pub fn to_frame(&self, proposal: ObjectId) -> Result<Frame, ProtocolError> {
        self.validate()?;
        Ok(Frame::new(FrameType::QsyAnswer, proposal, to_json(self, FrameType::QsyAnswer.max_payload())?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let a: QsyAnswer = from_json(&f.payload)?;
        a.validate()?;
        Ok(a)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfoWhat {
    Snr,
    Station,
}

/// INFO_REQUEST (0x40); the frame id is the request id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfoRequest {
    pub what: InfoWhat,
}

impl InfoRequest {
    pub fn to_frame(&self, request: ObjectId) -> Result<Frame, ProtocolError> {
        Ok(Frame::new(FrameType::InfoRequest, request, to_json(self, FrameType::InfoRequest.max_payload())?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        from_json(&f.payload)
    }
}

/// How well the answering station hears the asker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnrReport {
    pub db: f32,
    pub age_s: u32,
    pub source: String,
}

/// The station fields an operator chose to share (B10 limits).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StationShare {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qth: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rig: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antenna: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl StationShare {
    /// The B10 limit of each text field, in bytes (the locator is a Maidenhead grid).
    pub const LIMITS: [(&'static str, usize); 7] =
        [("name", 32), ("qth", 48), ("rig", 48), ("power", 16), ("antenna", 48), ("about", 256), ("email", 254)];

    pub fn validate(&self) -> Result<(), ProtocolError> {
        let fields = [&self.name, &self.qth, &self.rig, &self.power, &self.antenna, &self.about, &self.email];
        for ((field, max), v) in Self::LIMITS.iter().zip(fields) {
            check_opt(field, v, *max)?;
        }
        if self.locator.as_deref().is_some_and(|g| !crate::beacon::valid_grid(g)) {
            return Err(bad("locator is not a Maidenhead grid"));
        }
        Ok(())
    }
}

/// INFO_RESPONSE (0x41); the frame id is the request id, or `ObjectId::ZERO`
/// for an unsolicited share.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfoResponse {
    pub snr: Option<SnrReport>,
    pub station: Option<StationShare>,
    pub reason: Option<String>,
}

impl InfoResponse {
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if let Some(r) = &self.snr {
            if !r.db.is_finite() {
                return Err(bad("SNR is not a finite number"));
            }
            check_len("source", &r.source, 16)?;
        }
        if let Some(s) = &self.station {
            s.validate()?;
        }
        check_opt("reason", &self.reason, 32)?;
        if self.snr.is_none() && self.station.is_none() && self.reason.is_none() {
            return Err(bad("an info response with nothing in it"));
        }
        Ok(())
    }
    pub fn to_frame(&self, request: ObjectId) -> Result<Frame, ProtocolError> {
        self.validate()?;
        Ok(Frame::new(FrameType::InfoResponse, request, to_json(self, FrameType::InfoResponse.max_payload())?))
    }
    pub fn from_frame(f: &Frame) -> Result<Self, ProtocolError> {
        let r: InfoResponse = from_json(&f.payload)?;
        r.validate()?;
        Ok(r)
    }
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
        let o = FileOffer {
            name: "a.bin".into(),
            size: 1,
            sha256: "A".repeat(64),
            mime: None,
            encoding: None,
            wire_size: 1,
        };
        assert!(o.validate().is_err());
    }

    fn candidate(name: &str) -> QsyCandidate {
        QsyCandidate { freq_hz: 7_105_000, mode: "USB".into(), passband_hz: 2400, name: name.into() }
    }

    #[test]
    fn qsy_propose_limits() {
        let p = |n: usize, name: &str| QsyPropose { candidates: vec![candidate(name); n], note: None };
        assert!(p(0, "a").validate().is_err(), "no candidates");
        assert!(p(6, "a").validate().is_err(), "six candidates");
        assert!(p(5, "a").validate().is_ok());
        assert!(p(1, &"n".repeat(33)).validate().is_err(), "name of 33 bytes");
        assert!(p(1, &"n".repeat(32)).validate().is_ok());
        let mut bad_mode = p(1, "a");
        bad_mode.candidates[0].mode = "CW".into();
        assert!(bad_mode.validate().is_err());
        let mut no_freq = p(1, "a");
        no_freq.candidates[0].freq_hz = 0;
        assert!(no_freq.validate().is_err());
        let mut too_high = p(1, "a");
        too_high.candidates[0].freq_hz = 60_000_001;
        assert!(too_high.validate().is_err());
        let mut long_note = p(1, "a");
        long_note.note = Some("x".repeat(65));
        assert!(long_note.validate().is_err());
        assert!(QsyAnswer::Accept { index: 5 }.validate().is_err());
        assert!(QsyAnswer::Decline { reason: Some("r".repeat(33)) }.validate().is_err());
        let bad_grid = StationShare { locator: Some("ZZ99".into()), ..StationShare::default() };
        assert!(bad_grid.validate().is_err());
        let long_about = StationShare { about: Some("a".repeat(257)), ..StationShare::default() };
        assert!(long_about.validate().is_err());
    }

    #[test]
    fn unknown_json_field_refused() {
        let cases: [(FrameType, &[u8]); 4] = [
            (
                FrameType::QsyPropose,
                br#"{"candidates":[{"freq_hz":7105000,"mode":"","passband_hz":0,"name":""}],"note":null,"x":1}"#,
            ),
            (FrameType::QsyAnswer, br#"{"answer":"accept","index":0,"x":1}"#),
            (FrameType::InfoRequest, br#"{"what":"snr","x":1}"#),
            (FrameType::InfoResponse, br#"{"snr":null,"station":{"name":"a","x":1},"reason":null}"#),
        ];
        for (t, json) in cases {
            let f = Frame::new(t, ObjectId::ZERO, json.to_vec());
            let refused = match t {
                FrameType::QsyPropose => QsyPropose::from_frame(&f).is_err(),
                FrameType::QsyAnswer => QsyAnswer::from_frame(&f).is_err(),
                FrameType::InfoRequest => InfoRequest::from_frame(&f).is_err(),
                _ => InfoResponse::from_frame(&f).is_err(),
            };
            assert!(refused, "{t:?} took an unknown field");
        }
    }

    #[test]
    fn new_frames_roundtrip() {
        let p = QsyPropose { candidates: vec![candidate("a")], note: Some("n".into()) };
        assert_eq!(QsyPropose::from_frame(&p.to_frame(ObjectId::ZERO).unwrap()).unwrap(), p);
        let r = InfoResponse { snr: None, station: None, reason: Some("no-reading".into()) };
        assert_eq!(InfoResponse::from_frame(&r.to_frame(ObjectId::ZERO).unwrap()).unwrap(), r);
    }

    #[test]
    fn chunk_roundtrip() {
        let f = chunk_frame(ObjectId::ZERO, 4096, &[0xC0, 0xDB]).unwrap();
        let (o, d) = parse_chunk(&f).unwrap();
        assert_eq!((o, d), (4096, &[0xC0u8, 0xDB][..]));
    }

    // --- M4 C1: minor-2 offers ---

    fn offer(encoding: Option<Encoding>, size: u64, wire_size: u64) -> FileOffer {
        FileOffer {
            name: "a.txt".into(),
            size,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
            mime: None,
            encoding,
            wire_size,
        }
    }

    #[test]
    fn offer_plain_wire_size_must_equal_size() {
        assert!(offer(None, 10, 10).to_frame(ObjectId::ZERO).is_ok());
        assert!(offer(None, 10, 9).to_frame(ObjectId::ZERO).is_err());
        assert!(offer(Some(Encoding::Zstd), 10, 4).to_frame(ObjectId::ZERO).is_ok());
        assert!(offer(Some(Encoding::Zstd), 10, 0).to_frame(ObjectId::ZERO).is_err(), "wire_size at least 1");
    }

    #[test]
    fn unknown_encoding_rejected() {
        let json = br#"{"name":"a","size":3,"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad","encoding":"gzip","wire_size":2}"#;
        let f = Frame::new(FrameType::FileOffer, ObjectId::ZERO, json.to_vec());
        assert!(FileOffer::from_frame(&f).is_err());
    }

    #[test]
    fn offer_without_wire_size_rejected() {
        let json =
            br#"{"name":"a","size":3,"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}"#;
        let f = Frame::new(FrameType::FileOffer, ObjectId::ZERO, json.to_vec());
        assert!(FileOffer::from_frame(&f).is_err(), "the minor-0 shape is not a minor-2 offer");
    }

    /// Final review 1: an encoded file is only sent when it is smaller, so a
    /// wire size at or over the file's own is refused (it could also overflow
    /// the receiver's space check).
    #[test]
    fn zstd_wire_size_must_be_smaller() {
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string();
        let offer = |wire_size| FileOffer {
            name: "a.txt".into(),
            size: 100,
            sha256: sha.clone(),
            mime: None,
            encoding: Some(Encoding::Zstd),
            wire_size,
        };
        assert!(offer(99).validate().is_ok());
        assert!(offer(100).validate().is_err());
        assert!(offer(u64::MAX - 10).validate().is_err());
    }

    /// L2: the 160-byte cut must not split a character.
    #[test]
    fn a_long_reason_is_cut_between_characters() {
        for (lead, ch) in [("a", "é"), ("", "€"), ("a", "€"), ("ab", "🙂"), ("", "x")] {
            let text = format!("{lead}{}", ch.repeat(100));
            let r = Reason::new("code", &text);
            assert!(r.text.len() <= 160 && r.text.len() + ch.len() > 160.min(text.len()), "{} bytes", r.text.len());
            assert!(text.starts_with(&r.text));
        }
        assert_eq!(Reason::new("code", "short é").text, "short é");
    }
}
