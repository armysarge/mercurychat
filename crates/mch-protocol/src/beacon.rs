//! MCB/1 station beacon (docs/03 §6): a compact, self-delimiting payload sent
//! as a wrapped KISS message (command 0x00) on Mercury's broadcast plane.
//!
//! ```text
//!  offset size field
//!   0      4  magic "MCB1"
//!   4      1  version (1)
//!   5      1  kind (1 = beacon)
//!   6      4  sequence, big-endian (random start, +1 per beacon)
//!  10      1  callsign length n (1..=15)
//!  11      n  callsign: A-Z 0-9 '/' '-'
//!  11+n    1  grid length g (0, 4, 6 or 8)
//!  12+n    g  Maidenhead locator
//!  12+n+g  2  capability bits, big-endian
//! ```
//!
//! 14 fixed bytes plus callsign and grid: at most 37 bytes. The payload must
//! be exactly this long: Mercury silently truncates over-capacity broadcasts
//! and zero-padding is removed by its length prefix, so any length mismatch is
//! malformed. Anything without the magic is another application's traffic.

pub const MAGIC: [u8; 4] = *b"MCB1";
pub const VERSION: u8 = 1;
pub const KIND_BEACON: u8 = 1;
pub const FIXED_LEN: usize = 14;
pub const MAX_CALL: usize = 15;
pub const MAX_LEN: usize = FIXED_LEN + MAX_CALL + 8;

/// Capability bits. Unknown bits are preserved, never interpreted.
pub const CAP_CHAT: u16 = 0x0001;
pub const CAP_FILE: u16 = 0x0002;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Beacon {
    pub seq: u32,
    pub callsign: String,
    pub grid: Option<String>,
    pub caps: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BeaconError {
    #[error("not an MCB1 payload")]
    NotMcb,
    #[error("unsupported MCB version {0}")]
    Version(u8),
    #[error("unsupported MCB kind {0}")]
    Kind(u8),
    #[error("length mismatch: declared fields need {need} bytes, payload has {have}")]
    Length { need: usize, have: usize },
    #[error("invalid callsign")]
    Callsign,
    #[error("invalid locator")]
    Grid,
    #[error("invalid TLV field")]
    Tlv,
    #[error("invalid mode code {0}")]
    Mode(u8),
    #[error("{0} QSY candidates; 1 to 3 allowed")]
    Candidates(usize),
    #[error("{len} bytes is over the listen mode's {cap}-byte capacity")]
    TooLong { len: usize, cap: usize },
}

/// Classification of one received broadcast payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    Beacon(Beacon),
    /// Kind 2 (M3c B7).
    Extended(ExtendedBeacon),
    /// Kind 3 (M3c B5).
    Qsy(QsySuggestion),
    /// MCB1 magic and version 1, with a kind this version does not know:
    /// counted as unsupported, never shown as malformed (M3c B7).
    Unsupported(u8),
    /// Carries the MCB1 magic but fails validation: shown as malformed, never as a station.
    Malformed(BeaconError),
    /// Another application's broadcast: shown as unsupported activity at most.
    Foreign,
}

pub fn classify(payload: &[u8]) -> Received {
    let kind = match header(payload, None) {
        Ok(k) => k,
        Err(BeaconError::NotMcb) => return Received::Foreign,
        Err(e) => return Received::Malformed(e),
    };
    let got = match kind {
        KIND_BEACON => Beacon::decode(payload).map(Received::Beacon),
        KIND_EXTENDED => ExtendedBeacon::decode(payload).map(Received::Extended),
        KIND_QSY => QsySuggestion::decode(payload).map(Received::Qsy),
        k => return Received::Unsupported(k),
    };
    got.unwrap_or_else(Received::Malformed)
}

/// Magic, a length that holds the header, version 1, and (when given) the
/// kind; returns the kind.
fn header(p: &[u8], kind: impl Into<Option<u8>>) -> Result<u8, BeaconError> {
    if p.len() < 4 || p[..4] != MAGIC {
        return Err(BeaconError::NotMcb);
    }
    if p.len() < 10 {
        return Err(BeaconError::Length { need: 10, have: p.len() });
    }
    if p[4] != VERSION {
        return Err(BeaconError::Version(p[4]));
    }
    match kind.into() {
        Some(k) if p[5] != k => Err(BeaconError::Kind(p[5])),
        _ => Ok(p[5]),
    }
}

fn valid_call(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= MAX_CALL
        && c.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/' || b == b'-')
        && c.bytes().any(|b| b.is_ascii_alphanumeric())
}

/// Maidenhead locator of 4, 6 or 8 characters: AA00, AA00aa, AA00aa00
/// (subsquare letters accepted in either case and normalised to lower case).
pub fn valid_grid(g: &str) -> bool {
    let b = g.as_bytes();
    let field = |c: u8| (b'A'..=b'R').contains(&c.to_ascii_uppercase());
    let sub = |c: u8| (b'A'..=b'X').contains(&c.to_ascii_uppercase());
    match b.len() {
        4 | 6 | 8 => {
            field(b[0])
                && field(b[1])
                && b[2].is_ascii_digit()
                && b[3].is_ascii_digit()
                && (b.len() < 6 || (sub(b[4]) && sub(b[5])))
                && (b.len() < 8 || (b[6].is_ascii_digit() && b[7].is_ascii_digit()))
        }
        _ => false,
    }
}

/// Field upper case, subsquare lower case: `kg33xi` → `KG33xi`.
pub fn normalise_grid(g: &str) -> String {
    g.char_indices().map(|(i, c)| if i < 2 { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }).collect()
}

impl Beacon {
    pub fn new(seq: u32, callsign: &str, grid: Option<&str>, caps: u16) -> Result<Self, BeaconError> {
        let callsign = callsign.trim().to_ascii_uppercase();
        if !valid_call(&callsign) {
            return Err(BeaconError::Callsign);
        }
        let grid = match grid.map(str::trim).filter(|g| !g.is_empty()) {
            Some(g) if valid_grid(g) => Some(normalise_grid(g)),
            Some(_) => return Err(BeaconError::Grid),
            None => None,
        };
        Ok(Beacon { seq, callsign, grid, caps })
    }

    pub fn encoded_len(&self) -> usize {
        FIXED_LEN + self.callsign.len() + self.grid.as_ref().map_or(0, |g| g.len())
    }

    pub fn encode(&self) -> Vec<u8> {
        let grid = self.grid.as_deref().unwrap_or("");
        let mut v = Vec::with_capacity(self.encoded_len());
        v.extend_from_slice(&MAGIC);
        v.push(VERSION);
        v.push(KIND_BEACON);
        v.extend_from_slice(&self.seq.to_be_bytes());
        v.push(self.callsign.len() as u8);
        v.extend_from_slice(self.callsign.as_bytes());
        v.push(grid.len() as u8);
        v.extend_from_slice(grid.as_bytes());
        v.extend_from_slice(&self.caps.to_be_bytes());
        v
    }

    pub fn decode(p: &[u8]) -> Result<Self, BeaconError> {
        let (b, end) = Beacon::decode_body(p, KIND_BEACON)?;
        if p.len() != end {
            return Err(BeaconError::Length { need: end, have: p.len() });
        }
        Ok(b)
    }

    /// The kind 1 body at the start of a kind 1 or kind 2 payload, and where it ends.
    fn decode_body(p: &[u8], kind: u8) -> Result<(Self, usize), BeaconError> {
        header(p, kind)?;
        let short = |need| BeaconError::Length { need, have: p.len() };
        if p.len() < FIXED_LEN {
            return Err(short(FIXED_LEN));
        }
        let seq = u32::from_be_bytes([p[6], p[7], p[8], p[9]]);
        let n = p[10] as usize;
        if p.len() < 11 + n + 1 {
            return Err(short(FIXED_LEN + n));
        }
        let g = p[11 + n] as usize;
        let end = FIXED_LEN + n + g;
        if p.len() < end {
            return Err(short(end));
        }
        let callsign = std::str::from_utf8(&p[11..11 + n]).map_err(|_| BeaconError::Callsign)?;
        if !valid_call(callsign) {
            return Err(BeaconError::Callsign);
        }
        let grid = if g == 0 {
            None
        } else {
            let s = std::str::from_utf8(&p[12 + n..12 + n + g]).map_err(|_| BeaconError::Grid)?;
            if !valid_grid(s) {
                return Err(BeaconError::Grid);
            }
            Some(normalise_grid(s))
        };
        let caps = u16::from_be_bytes([p[end - 2], p[end - 1]]);
        Ok((Beacon { seq, callsign: callsign.to_string(), grid, caps }, end))
    }
}

// ---- Kinds 2 and 3 (M3c B7, B9) ----

pub const KIND_EXTENDED: u8 = 2;
pub const KIND_QSY: u8 = 3;
/// Kind 2: the kind 1 fixed bytes plus the TLV count.
pub const EXTENDED_FIXED_LEN: usize = FIXED_LEN + 1;
pub const MAX_TLVS: usize = 8;
pub const MAX_TLV_LEN: usize = 40;
pub const MAX_QSY_CANDIDATES: usize = 3;

/// Wrapped capacity of each runtime-selectable listen mode: Mercury's frame
/// bytes less the 3-byte wrap (copied from mercury-adapter's kiss.rs, for the
/// pinned build; a contract test checks the two tables match).
pub fn listen_capacity(listen_mode: u8) -> Option<usize> {
    let frame = match listen_mode {
        0 => 510,
        1 => 126,
        3 => 54,
        7 => 30,
        9 => 1180,
        10 => 1213,
        11 | 12 => 98,
        _ => return None,
    };
    Some(frame - 3)
}

/// Whether a payload of `payload_len` bytes survives intact on the listen mode.
pub fn fits(payload_len: usize, listen_mode: u8) -> bool {
    listen_capacity(listen_mode).is_some_and(|cap| payload_len <= cap)
}

fn check_fits(bytes: Vec<u8>, listen_mode: u8) -> Result<Vec<u8>, BeaconError> {
    let cap = listen_capacity(listen_mode).unwrap_or(0);
    if bytes.len() > cap {
        return Err(BeaconError::TooLong { len: bytes.len(), cap });
    }
    Ok(bytes)
}

/// One TLV field of an extended beacon (UTF-8 values).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tlv {
    /// Type 1, at most 24 bytes.
    Name(String),
    /// Type 2, at most 24 bytes.
    Qth(String),
    /// Type 3, at most 40 bytes.
    Status(String),
    /// A type this version does not know: kept on decode, skipped by consumers.
    Unknown(u8, Vec<u8>),
}

impl Tlv {
    fn parts(&self) -> (u8, &[u8], usize) {
        match self {
            Tlv::Name(s) => (1, s.as_bytes(), 24),
            Tlv::Qth(s) => (2, s.as_bytes(), 24),
            Tlv::Status(s) => (3, s.as_bytes(), 40),
            Tlv::Unknown(t, v) => (*t, v, MAX_TLV_LEN),
        }
    }

    fn valid(&self) -> bool {
        let (t, v, max) = self.parts();
        !(1..=3).contains(&t) == matches!(self, Tlv::Unknown(..)) && (1..=max).contains(&v.len())
    }

    fn decode(t: u8, v: &[u8]) -> Result<Tlv, BeaconError> {
        let text = || std::str::from_utf8(v).map(str::to_string).map_err(|_| BeaconError::Tlv);
        let tlv = match t {
            1 => Tlv::Name(text()?),
            2 => Tlv::Qth(text()?),
            3 => Tlv::Status(text()?),
            _ => Tlv::Unknown(t, v.to_vec()),
        };
        if tlv.valid() {
            Ok(tlv)
        } else {
            Err(BeaconError::Tlv)
        }
    }
}

/// MCB/1 kind 2: a beacon with TLV fields after the kind 1 body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedBeacon {
    pub beacon: Beacon,
    pub tlvs: Vec<Tlv>,
}

impl ExtendedBeacon {
    pub fn new(beacon: Beacon, tlvs: Vec<Tlv>) -> Result<Self, BeaconError> {
        if tlvs.len() > MAX_TLVS || !tlvs.iter().all(Tlv::valid) {
            return Err(BeaconError::Tlv);
        }
        Ok(ExtendedBeacon { beacon, tlvs })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = self.beacon.encode();
        v[5] = KIND_EXTENDED;
        v.push(self.tlvs.len() as u8);
        for tlv in &self.tlvs {
            let (t, value, _) = tlv.parts();
            v.push(t);
            v.push(value.len() as u8);
            v.extend_from_slice(value);
        }
        v
    }

    /// The encoding, refused when it is over the listen mode's capacity.
    pub fn encode_for(&self, listen_mode: u8) -> Result<Vec<u8>, BeaconError> {
        check_fits(self.encode(), listen_mode)
    }

    pub fn decode(p: &[u8]) -> Result<Self, BeaconError> {
        let (beacon, mut at) = Beacon::decode_body(p, KIND_EXTENDED)?;
        let short = |need| BeaconError::Length { need, have: p.len() };
        let count = *p.get(at).ok_or(short(at + 1))? as usize;
        at += 1;
        if count > MAX_TLVS {
            return Err(BeaconError::Tlv);
        }
        let mut tlvs = Vec::with_capacity(count);
        for _ in 0..count {
            let (&t, &len) = (p.get(at).ok_or(short(at + 2))?, p.get(at + 1).ok_or(short(at + 2))?);
            let len = len as usize;
            if len == 0 || len > MAX_TLV_LEN {
                return Err(BeaconError::Tlv);
            }
            let value = p.get(at + 2..at + 2 + len).ok_or(short(at + 2 + len))?;
            tlvs.push(Tlv::decode(t, value)?);
            at += 2 + len;
        }
        if p.len() != at {
            return Err(short(at));
        }
        Ok(ExtendedBeacon { beacon, tlvs })
    }
}

/// A rig mode in a QSY suggestion (B9 codes 0..=6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Unspecified,
    Usb,
    Lsb,
    PktUsb,
    PktLsb,
    Fm,
    PktFm,
}

impl Mode {
    const ALL: [(Mode, &'static str); 7] = [
        (Mode::Unspecified, ""),
        (Mode::Usb, "USB"),
        (Mode::Lsb, "LSB"),
        (Mode::PktUsb, "PKTUSB"),
        (Mode::PktLsb, "PKTLSB"),
        (Mode::Fm, "FM"),
        (Mode::PktFm, "PKTFM"),
    ];

    pub fn code(self) -> u8 {
        Mode::ALL.iter().position(|(m, _)| *m == self).unwrap() as u8
    }
    pub fn from_code(c: u8) -> Option<Mode> {
        Mode::ALL.get(c as usize).map(|(m, _)| *m)
    }
    /// The rig mode name (as Hamlib and the frequency list spell it); "" for unspecified.
    pub fn name(self) -> &'static str {
        Mode::ALL[self.code() as usize].1
    }
    pub fn from_name(s: &str) -> Option<Mode> {
        Mode::ALL.iter().find(|(_, n)| *n == s).map(|(m, _)| *m)
    }
}

/// MCB/1 kind 3: a QSY suggestion from one station to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QsySuggestion {
    /// The sender's beacon sequence counter.
    pub seq: u32,
    pub from: String,
    pub to: String,
    /// 1..=3 (frequency Hz, mode).
    pub candidates: Vec<(u32, Mode)>,
}

impl QsySuggestion {
    pub fn new(seq: u32, from: &str, to: &str, candidates: Vec<(u32, Mode)>) -> Result<Self, BeaconError> {
        let (from, to) = (from.trim().to_ascii_uppercase(), to.trim().to_ascii_uppercase());
        if !valid_call(&from) || !valid_call(&to) {
            return Err(BeaconError::Callsign);
        }
        if !(1..=MAX_QSY_CANDIDATES).contains(&candidates.len()) {
            return Err(BeaconError::Candidates(candidates.len()));
        }
        Ok(QsySuggestion { seq, from, to, candidates })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(13 + self.from.len() + self.to.len() + 5 * self.candidates.len());
        v.extend_from_slice(&MAGIC);
        v.push(VERSION);
        v.push(KIND_QSY);
        v.extend_from_slice(&self.seq.to_be_bytes());
        for call in [&self.from, &self.to] {
            v.push(call.len() as u8);
            v.extend_from_slice(call.as_bytes());
        }
        v.push(self.candidates.len() as u8);
        for (hz, mode) in &self.candidates {
            v.extend_from_slice(&hz.to_be_bytes());
            v.push(mode.code());
        }
        v
    }

    /// The encoding, refused when it is over the listen mode's capacity.
    pub fn encode_for(&self, listen_mode: u8) -> Result<Vec<u8>, BeaconError> {
        check_fits(self.encode(), listen_mode)
    }

    pub fn decode(p: &[u8]) -> Result<Self, BeaconError> {
        header(p, KIND_QSY)?;
        let short = |need| BeaconError::Length { need, have: p.len() };
        let seq = u32::from_be_bytes([p[6], p[7], p[8], p[9]]);
        let mut at = 10;
        let mut calls = Vec::with_capacity(2);
        for _ in 0..2 {
            let n = *p.get(at).ok_or(short(at + 1))? as usize;
            let raw = p.get(at + 1..at + 1 + n).ok_or(short(at + 1 + n))?;
            let call = std::str::from_utf8(raw).map_err(|_| BeaconError::Callsign)?;
            if !valid_call(call) {
                return Err(BeaconError::Callsign);
            }
            calls.push(call.to_string());
            at += 1 + n;
        }
        let c = *p.get(at).ok_or(short(at + 1))? as usize;
        at += 1;
        if !(1..=MAX_QSY_CANDIDATES).contains(&c) {
            return Err(BeaconError::Candidates(c));
        }
        if p.len() != at + 5 * c {
            return Err(short(at + 5 * c));
        }
        let mut candidates = Vec::with_capacity(c);
        for &[a, b, c, d, mode] in p[at..].as_chunks::<5>().0 {
            candidates.push((u32::from_be_bytes([a, b, c, d]), Mode::from_code(mode).ok_or(BeaconError::Mode(mode))?));
        }
        let to = calls.pop().unwrap();
        let from = calls.pop().unwrap();
        Ok(QsySuggestion { seq, from, to, candidates })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_sizes() {
        let b = Beacon::new(0x01020304, "zs6abc/p", Some("kg43ab"), CAP_CHAT | CAP_FILE).unwrap();
        assert_eq!(b.callsign, "ZS6ABC/P");
        assert_eq!(b.grid.as_deref(), Some("KG43ab"));
        let e = b.encode();
        assert_eq!(e.len(), b.encoded_len());
        assert_eq!(Beacon::decode(&e).unwrap(), b);
        let max = Beacon::new(u32::MAX, "ABCDEFGHIJKLMNO", Some("AA00aa00"), u16::MAX).unwrap();
        assert_eq!(max.encode().len(), MAX_LEN);
        let min = Beacon::new(0, "K1", None, 0).unwrap();
        assert_eq!(Beacon::decode(&min.encode()).unwrap(), min);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(Beacon::new(1, "", None, 0).is_err());
        assert!(Beacon::new(1, "BAD CALL", None, 0).is_err());
        assert!(Beacon::new(1, "K1ABC", Some("ZZ99"), 0).is_err());
        assert!(Beacon::new(1, "K1ABC", Some("AA0"), 0).is_err());
        let good = Beacon::new(7, "K1ABC", Some("FN42"), CAP_CHAT).unwrap().encode();
        // Truncated (as Mercury does to over-capacity broadcasts) and padded.
        for cut in 0..good.len() {
            assert!(Beacon::decode(&good[..cut]).is_err(), "cut {cut}");
        }
        let mut padded = good.clone();
        padded.push(0);
        assert!(matches!(Beacon::decode(&padded), Err(BeaconError::Length { .. })));
        let mut v2 = good.clone();
        v2[4] = 2;
        assert_eq!(Beacon::decode(&v2), Err(BeaconError::Version(2)));
        let mut lower = good.clone();
        lower[11] = b'k';
        assert_eq!(Beacon::decode(&lower), Err(BeaconError::Callsign));
    }

    fn base() -> Beacon {
        Beacon::new(9, "ZS6ABC", Some("KG44dg"), CAP_CHAT).unwrap()
    }

    #[test]
    fn unknown_kind_is_unsupported() {
        let mut p = base().encode();
        p[5] = 9;
        assert_eq!(classify(&p), Received::Unsupported(9));
        p[4] = 2;
        assert!(matches!(classify(&p), Received::Malformed(BeaconError::Version(2))), "a new version is malformed");
    }

    #[test]
    fn unknown_tlv_skipped() {
        let tlvs = vec![Tlv::Name("Pat".into()), Tlv::Unknown(9, vec![1, 2, 3]), Tlv::Status("QRV 40m".into())];
        let e = ExtendedBeacon::new(base(), tlvs.clone()).unwrap();
        let Received::Extended(got) = classify(&e.encode()) else { panic!("not extended") };
        assert_eq!(got.tlvs, tlvs, "the unknown TLV is kept on decode");
        assert_eq!(got.beacon, base());
        let known: Vec<_> = got.tlvs.iter().filter(|t| !matches!(t, Tlv::Unknown(..))).collect();
        assert_eq!(known.len(), 2);
    }

    #[test]
    fn tlv_limits_and_lengths() {
        assert!(ExtendedBeacon::new(base(), vec![Tlv::Name("n".repeat(25))]).is_err());
        assert!(ExtendedBeacon::new(base(), vec![Tlv::Qth("q".repeat(25))]).is_err());
        assert!(ExtendedBeacon::new(base(), vec![Tlv::Status("s".repeat(41))]).is_err());
        assert!(ExtendedBeacon::new(base(), vec![Tlv::Status(String::new())]).is_err(), "empty TLV");
        assert!(ExtendedBeacon::new(base(), vec![Tlv::Name("n".into()); 9]).is_err(), "nine TLVs");
        let good = ExtendedBeacon::new(base(), vec![Tlv::Status("QRV".into())]).unwrap().encode();
        for cut in 0..good.len() {
            assert!(!matches!(classify(&good[..cut]), Received::Extended(_)), "cut {cut}");
        }
        let mut padded = good.clone();
        padded.push(0);
        assert!(matches!(classify(&padded), Received::Malformed(BeaconError::Length { .. })));
        let mut zero_len = good.clone();
        let at = zero_len.len() - 4; // the status TLV's length byte
        zero_len[at] = 0;
        zero_len.truncate(at + 1);
        assert!(matches!(classify(&zero_len), Received::Malformed(_)));
    }

    #[test]
    fn bad_mode_code_malformed() {
        let q = QsySuggestion::new(1, "ZS6ABC", "N0CALL", vec![(7_105_000, Mode::Usb)]).unwrap();
        let mut p = q.encode();
        *p.last_mut().unwrap() = 7;
        assert_eq!(classify(&p), Received::Malformed(BeaconError::Mode(7)));
        assert_eq!(Mode::from_name("PKTUSB"), Some(Mode::PktUsb));
        assert_eq!(Mode::from_name(""), Some(Mode::Unspecified));
        assert_eq!(Mode::from_name("CW"), None);
        assert_eq!(Mode::PktFm.name(), "PKTFM");
        assert!(QsySuggestion::new(1, "ZS6ABC", "N0CALL", vec![]).is_err());
        assert!(QsySuggestion::new(1, "ZS6ABC", "N0CALL", vec![(7_105_000, Mode::Usb); 4]).is_err());
        assert!(QsySuggestion::new(1, "ZS6ABC", "bad call", vec![(7_105_000, Mode::Usb)]).is_err());
    }

    #[test]
    fn kind2_all_fields_is_132_bytes() {
        // B7/B9: 15 bytes before the TLVs + call 15 + grid 8 + (2+24) + (2+24) + (2+40).
        let b = Beacon::new(u32::MAX, "ABCDEFGHIJKLMNO", Some("RR99xx99"), u16::MAX).unwrap();
        let tlvs = vec![Tlv::Name("n".repeat(24)), Tlv::Qth("q".repeat(24)), Tlv::Status("s".repeat(40))];
        let e = ExtendedBeacon::new(b, tlvs).unwrap();
        assert_eq!(e.encode().len(), 15 + 15 + 8 + 26 + 26 + 42);
        assert_eq!(e.encode().len(), 132);
        assert!(matches!(e.encode_for(1), Err(BeaconError::TooLong { len: 132, cap: 123 })));
        let q = QsySuggestion::new(u32::MAX, "ABCDEFGHIJKLMNO", "ABCDEFGHIJKLMNO", vec![(60_000_000, Mode::PktFm); 3]);
        assert_eq!(q.unwrap().encode().len(), 58);
    }

    #[test]
    fn fits_datac3_123() {
        assert!(fits(123, 1));
        assert!(!fits(124, 1));
        assert!(!fits(1, 2), "an unknown listen mode carries nothing");
        assert_eq!(listen_capacity(1), Some(123));
    }

    #[test]
    fn classifies_foreign_traffic() {
        assert_eq!(classify(b"hello from reticulum"), Received::Foreign);
        assert_eq!(classify(b""), Received::Foreign);
        assert!(matches!(classify(b"MCB1\x01"), Received::Malformed(_)));
    }
}
