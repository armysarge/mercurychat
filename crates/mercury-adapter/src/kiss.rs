//! KISS framing for Mercury's broadcast port (docs/03 §6; Mercury
//! `docs/TNC.md` "Broadcast Port"). FEND-delimited frames, first byte is the
//! KISS command, payload escaped C0 -> DB DC and DB -> DB DD.
//!
//! The decoder is incremental and bounded: split and coalesced TCP reads,
//! empty frames, malformed escapes and oversized frames are all handled
//! without unbounded buffering. A bad frame is dropped and counted; decoding
//! resumes at the next FEND.

pub const FEND: u8 = 0xC0;
pub const FESC: u8 = 0xDB;
pub const TFEND: u8 = 0xDC;
pub const TFESC: u8 = 0xDD;

/// Wrapped message: Mercury adds its 1-byte header and 2-byte length.
pub const CMD_DATA_STD: u8 = 0x00;
/// Raw modem frame, transmitted untouched (station messages, RaptorQ).
pub const CMD_MODEM_FRAME: u8 = 0x03;

/// Largest unescaped frame (command + payload) the decoder keeps. Mercury's
/// largest listen-mode frame is DATAC17's 1184 bytes at the pinned build.
pub const MAX_FRAME: usize = 2048;

/// Bytes Mercury adds to a wrapped (0x00/0x01/unformatted 0x02) message.
pub const WRAP_OVERHEAD: usize = 3;

/// Payload bytes per modem frame for the runtime-selectable listen modes, as
/// printed by `mercury -l` for the pinned build (Mercury 1.9.25, 33efac4).
/// Mode index -> (name, frame bytes). Unknown index: broadcast TX disabled.
pub fn listen_mode_frame(index: u8) -> Option<(&'static str, usize)> {
    Some(match index {
        0 => ("DATAC1", 510),
        1 => ("DATAC3", 126),
        3 => ("DATAC4", 54),
        7 => ("DATAC15", 30),
        9 => ("DATAC17", 1184),
        10 => ("QAM16C2", 1183),
        11 => ("MFSK", 98),
        12 => ("MFSK500", 98),
        _ => return None,
    })
}

/// Largest wrapped-message payload that survives intact on a listen mode.
/// Mercury silently truncates anything longer (lab finding, 15 Nov 2026).
pub fn wrapped_capacity(listen_mode: u8) -> Option<usize> {
    listen_mode_frame(listen_mode).map(|(_, f)| f - WRAP_OVERHEAD)
}

pub fn encode(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.push(FEND);
    out.push(cmd);
    for &b in payload {
        match b {
            FEND => out.extend_from_slice(&[FESC, TFEND]),
            FESC => out.extend_from_slice(&[FESC, TFESC]),
            _ => out.push(b),
        }
    }
    out.push(FEND);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KissFrame {
    pub cmd: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KissStats {
    pub frames: u64,
    pub bad_escape: u64,
    pub oversized: u64,
}

#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
    in_frame: bool,
    escaped: bool,
    discard: bool,
    pub stats: KissStats,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<KissFrame> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == FEND {
                if self.in_frame && !self.discard && !self.escaped && !self.buf.is_empty() {
                    let cmd = self.buf[0];
                    out.push(KissFrame { cmd, payload: self.buf[1..].to_vec() });
                    self.stats.frames += 1;
                } else if self.escaped && !self.discard {
                    self.stats.bad_escape += 1;
                }
                self.buf.clear();
                self.in_frame = true;
                self.escaped = false;
                self.discard = false;
                continue;
            }
            if !self.in_frame || self.discard {
                continue;
            }
            let byte = if self.escaped {
                self.escaped = false;
                match b {
                    TFEND => FEND,
                    TFESC => FESC,
                    _ => {
                        self.stats.bad_escape += 1;
                        self.discard = true;
                        continue;
                    }
                }
            } else if b == FESC {
                self.escaped = true;
                continue;
            } else {
                b
            };
            if self.buf.len() >= MAX_FRAME {
                self.stats.oversized += 1;
                self.discard = true;
                self.buf.clear();
                continue;
            }
            self.buf.push(byte);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table matches `mercury -l` at the pin, for every mode it lists
    /// (`tests/fixtures/mercury-listen-modes.txt`, saved from the pinned build).
    #[test]
    fn listen_mode_sizes_match_the_pinned_build() {
        let fixture = include_str!("../../../tests/fixtures/mercury-listen-modes.txt");
        for line in fixture.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let f: Vec<&str> = line.split_whitespace().collect();
            let (index, name, bytes): (u8, &str, usize) = (f[0].parse().unwrap(), f[1], f[2].parse().unwrap());
            if let Some((n, b)) = listen_mode_frame(index) {
                assert_eq!((n, b), (name, bytes), "mode {index}");
            }
        }
    }

    #[test]
    fn round_trip_with_escapes_and_split_reads() {
        let payload = vec![0x00, FEND, 0x41, FESC, TFEND, TFESC, 0xFF];
        let wire = encode(CMD_DATA_STD, &payload);
        assert_eq!(wire, vec![FEND, 0x00, 0x00, FESC, TFEND, 0x41, FESC, TFESC, TFEND, TFESC, 0xFF, FEND]);
        for split in 0..wire.len() {
            let mut d = Decoder::new();
            let mut got = d.push(&wire[..split]);
            got.extend(d.push(&wire[split..]));
            assert_eq!(got, vec![KissFrame { cmd: 0, payload: payload.clone() }], "split {split}");
        }
    }

    #[test]
    fn coalesced_empty_and_garbage() {
        let mut wire = vec![0x99, 0x98]; // noise before the first FEND is ignored
        wire.extend(encode(0x00, b"one"));
        wire.extend([FEND, FEND, FEND]); // empty frames
        wire.extend(encode(0x03, b"two"));
        let mut d = Decoder::new();
        let got = d.push(&wire);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1], KissFrame { cmd: 3, payload: b"two".to_vec() });
    }

    #[test]
    fn bad_escape_and_oversized_are_dropped_then_resync() {
        let mut d = Decoder::new();
        let mut wire = vec![FEND, 0x00, b'a', FESC, 0x42, b'b', FEND];
        wire.extend(encode(0x00, b"ok"));
        let got = d.push(&wire);
        assert_eq!(got, vec![KissFrame { cmd: 0, payload: b"ok".to_vec() }]);
        assert_eq!(d.stats.bad_escape, 1);

        let mut big = vec![FEND, 0x00];
        big.extend(std::iter::repeat_n(0x41, MAX_FRAME + 10));
        big.push(FEND);
        big.extend(encode(0x00, b"after"));
        let got = d.push(&big);
        assert_eq!(got, vec![KissFrame { cmd: 0, payload: b"after".to_vec() }]);
        assert_eq!(d.stats.oversized, 1);
    }

    #[test]
    fn capacities_match_pinned_build() {
        assert_eq!(wrapped_capacity(1), Some(123)); // DATAC3, lab-verified 15 Nov 2026
        assert_eq!(wrapped_capacity(11), Some(95));
        assert_eq!(wrapped_capacity(8), None); // DATAC16 is the control mode, not a listen mode
    }
}
