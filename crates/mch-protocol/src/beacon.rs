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
}

/// Classification of one received broadcast payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    Beacon(Beacon),
    /// Carries the MCB1 magic but fails validation: shown as malformed, never as a station.
    Malformed(BeaconError),
    /// Another application's broadcast: shown as unsupported activity at most.
    Foreign,
}

pub fn classify(payload: &[u8]) -> Received {
    match Beacon::decode(payload) {
        Ok(b) => Received::Beacon(b),
        Err(BeaconError::NotMcb) => Received::Foreign,
        Err(e) => Received::Malformed(e),
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

fn normalise_grid(g: &str) -> String {
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
        if p.len() < 4 || p[..4] != MAGIC {
            return Err(BeaconError::NotMcb);
        }
        let short = |need| BeaconError::Length { need, have: p.len() };
        if p.len() < FIXED_LEN {
            return Err(short(FIXED_LEN));
        }
        if p[4] != VERSION {
            return Err(BeaconError::Version(p[4]));
        }
        if p[5] != KIND_BEACON {
            return Err(BeaconError::Kind(p[5]));
        }
        let seq = u32::from_be_bytes([p[6], p[7], p[8], p[9]]);
        let n = p[10] as usize;
        if p.len() < 11 + n + 1 {
            return Err(short(FIXED_LEN + n));
        }
        let g = p[11 + n] as usize;
        let need = FIXED_LEN + n + g;
        if p.len() != need {
            return Err(short(need));
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
        let caps = u16::from_be_bytes([p[need - 2], p[need - 1]]);
        Ok(Beacon { seq, callsign: callsign.to_string(), grid, caps })
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

    #[test]
    fn classifies_foreign_traffic() {
        assert_eq!(classify(b"hello from reticulum"), Received::Foreign);
        assert_eq!(classify(b""), Received::Foreign);
        assert!(matches!(classify(b"MCB1\x01"), Received::Malformed(_)));
    }
}
