//! Mercury control-port line protocol: CR-terminated ASCII (docs/03 §3).

/// Longest control line accepted before the stream is treated as faulty.
pub const MAX_LINE: usize = 4096;

/// Incremental CR-delimited line splitter. Tolerates CRLF (no empty event for
/// the LF), split and coalesced reads, and bounds the line length.
#[derive(Debug, Default)]
pub struct LineParser {
    buf: Vec<u8>,
    skip_lf: bool,
}

#[derive(Debug, PartialEq, Eq, Clone, thiserror::Error)]
#[error("control line longer than {MAX_LINE} bytes")]
pub struct LineTooLong;

impl LineParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes and return complete, non-empty lines (lossy UTF-8).
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, LineTooLong> {
        let mut out = Vec::new();
        for &b in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => {
                    self.skip_lf = true;
                    self.flush(&mut out);
                }
                // A bare LF also ends a line (defensive; Mercury sends CR).
                b'\n' => self.flush(&mut out),
                _ => {
                    if self.buf.len() >= MAX_LINE {
                        self.buf.clear();
                        return Err(LineTooLong);
                    }
                    self.buf.push(b);
                }
            }
        }
        Ok(out)
    }

    fn flush(&mut self, out: &mut Vec<String>) {
        if !self.buf.is_empty() {
            out.push(String::from_utf8_lossy(&self.buf).into_owned());
            self.buf.clear();
        }
    }
}

/// Parsed control-port line. Command acknowledgements (OK/WRONG) are distinct
/// from asynchronous radio events; neither implies RF completion.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlEvent {
    Ok,
    Wrong,
    Registered(String),
    /// `CONNECTED <source> <dest> <bw>` — source is always the initiator.
    Connected {
        source: String,
        dest: String,
        bandwidth: String,
    },
    Disconnected,
    Pending,
    CancelPending,
    Buffer(u64),
    Ptt(bool),
    Sn(String),
    Bitrate(String),
    IAmAlive,
    Busy(bool),
    CqFrame {
        source: String,
        bandwidth: String,
    },
    Monitor(String),
    /// Response to VERSION; a fixed VARA string, not Mercury's identity.
    Version(String),
    /// `TUNE <level>`: the answer to `TUNE ?` (M3b).
    Tune(String),
    Unknown(String),
}

pub fn parse_event(line: &str) -> ControlEvent {
    let mut it = line.split_whitespace();
    let head = it.next().unwrap_or("");
    let rest: Vec<&str> = it.collect();
    match (head, rest.as_slice()) {
        ("OK", []) => ControlEvent::Ok,
        ("TUNE", [level]) => ControlEvent::Tune((*level).to_string()),
        ("WRONG", []) => ControlEvent::Wrong,
        ("REGISTERED", [c]) => ControlEvent::Registered((*c).to_string()),
        ("CONNECTED", [s, d, bw]) => {
            ControlEvent::Connected { source: (*s).to_string(), dest: (*d).to_string(), bandwidth: (*bw).to_string() }
        }
        ("DISCONNECTED", []) => ControlEvent::Disconnected,
        ("PENDING", []) => ControlEvent::Pending,
        ("CANCELPENDING", []) => ControlEvent::CancelPending,
        ("BUFFER", [n]) => n.parse().map(ControlEvent::Buffer).unwrap_or_else(|_| ControlEvent::Unknown(line.into())),
        ("PTT", ["ON"]) => ControlEvent::Ptt(true),
        ("PTT", ["OFF"]) => ControlEvent::Ptt(false),
        ("SN", [v]) => ControlEvent::Sn((*v).to_string()),
        ("BITRATE", _) if !rest.is_empty() => ControlEvent::Bitrate(rest.join(" ")),
        ("IAMALIVE", []) => ControlEvent::IAmAlive,
        ("BUSY", ["ON"]) => ControlEvent::Busy(true),
        ("BUSY", ["OFF"]) => ControlEvent::Busy(false),
        ("CQFRAME", [s, bw]) => ControlEvent::CqFrame { source: (*s).to_string(), bandwidth: (*bw).to_string() },
        ("MONITOR", _) => ControlEvent::Monitor(rest.join(" ")),
        ("VARA", _) | ("VERSION", ["VARA", ..]) => ControlEvent::Version(line.into()),
        _ => ControlEvent::Unknown(line.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_coalesced_crlf() {
        let stream = b"OK\r\nREGISTERED N0CALL\rCONNECTED A B 2300\r\nBUFFER 12\r";
        for split in 0..=stream.len() {
            let mut p = LineParser::new();
            let mut lines = p.push(&stream[..split]).unwrap();
            lines.extend(p.push(&stream[split..]).unwrap());
            assert_eq!(lines, vec!["OK", "REGISTERED N0CALL", "CONNECTED A B 2300", "BUFFER 12"], "split {split}");
        }
    }

    #[test]
    fn too_long_line() {
        let mut p = LineParser::new();
        assert_eq!(p.push(&vec![b'A'; MAX_LINE + 1]), Err(LineTooLong));
    }

    #[test]
    fn events() {
        assert_eq!(
            parse_event("CONNECTED AAAA BBBB 500"),
            ControlEvent::Connected { source: "AAAA".into(), dest: "BBBB".into(), bandwidth: "500".into() }
        );
        assert_eq!(parse_event("PTT ON"), ControlEvent::Ptt(true));
        assert_eq!(parse_event("BUFFER 0"), ControlEvent::Buffer(0));
        assert_eq!(parse_event("BUFFER x"), ControlEvent::Unknown("BUFFER x".into()));
        assert_eq!(parse_event("BITRATE (3) 1200 BPS"), ControlEvent::Bitrate("(3) 1200 BPS".into()));
        assert!(matches!(parse_event("VARA version 4.9.0 registered"), ControlEvent::Version(_)));
        // 1.9.25 answers VERSION with a second line, for Pat (#638).
        assert!(matches!(parse_event("VERSION VARA HF v4.9.0"), ControlEvent::Version(_)));
        assert_eq!(parse_event("SOMETHING NEW"), ControlEvent::Unknown("SOMETHING NEW".into()));
    }

    /// A peer name with spaces, or a missing one, is never a CONNECTED event:
    /// only three single-token fields are.
    #[test]
    fn connected_needs_exactly_three_tokens() {
        for line in ["CONNECTED ZS6ABC LISTEN ON ZS6ABC 2300", "CONNECTED  ZS6ABC 2300", "CONNECTED ZS6"] {
            assert_eq!(parse_event(line), ControlEvent::Unknown(line.into()), "{line:?}");
        }
    }

    /// A CR or LF inside a reported callsign ends the line there.
    #[test]
    fn line_breaks_split_a_reported_callsign() {
        let mut p = LineParser::new();
        let lines = p.push(b"CONNECTED ZS6\r\nABC ZS6ABC 2300\r").unwrap();
        assert_eq!(lines, ["CONNECTED ZS6", "ABC ZS6ABC 2300"]);
        assert!(lines.iter().all(|l| !matches!(parse_event(l), ControlEvent::Connected { .. })));
    }

    /// The buffer never grows past MAX_LINE, and the parser keeps working after.
    #[test]
    fn too_long_line_then_recovers() {
        let mut p = LineParser::new();
        assert_eq!(p.push(&vec![b'A'; 10 * MAX_LINE]), Err(LineTooLong));
        assert!(p.buf.len() <= MAX_LINE);
        assert_eq!(p.push(b"\rOK\r").unwrap(), ["OK"]);
    }
}
