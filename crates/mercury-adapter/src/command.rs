//! Control commands with validated arguments. Callsigns cannot carry CR/LF or
//! whitespace into the command line (injection guard, docs/03 §3).

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Callsign(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid callsign {0:?}: 1-15 chars of A-Z, 0-9, '/', '-'")]
pub struct BadCallsign(pub String);

impl Callsign {
    /// Normalise to upper case and validate syntax. Syntax validity is not
    /// licence verification.
    pub fn parse(s: &str) -> Result<Self, BadCallsign> {
        let up = s.trim().to_ascii_uppercase();
        let ok = !up.is_empty()
            && up.len() <= 15
            && up.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/' || b == b'-')
            && up.bytes().any(|b| b.is_ascii_alphanumeric());
        if ok && up == s.trim().to_ascii_uppercase() && !s.contains(['\r', '\n', ' ', '\t']) {
            Ok(Callsign(up))
        } else {
            Err(BadCallsign(s.to_string()))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Callsign {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bandwidth {
    Bw500,
    Bw2300,
    Bw2750,
}

impl Bandwidth {
    pub fn token(self) -> &'static str {
        match self {
            Bandwidth::Bw500 => "500",
            Bandwidth::Bw2300 => "2300",
            Bandwidth::Bw2750 => "2750",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    MyCall(Callsign),
    Listen(bool),
    Public(bool),
    Bw(Bandwidth),
    Connect {
        mycall: Callsign,
        theircall: Callsign,
    },
    Disconnect,
    Abort,
    CqFrame {
        source: Callsign,
        bw: Bandwidth,
    },
    /// Listen mode index (`MODE n`). Mercury answers a bare `BUSY` while an ARQ
    /// session is up, so only send it while the link is idle.
    Mode(u8),
    BusyDetect(bool),
    Monitor(bool),
    /// Mercury's tuning carrier (TNC.md "TUNE"): a 1 kHz tone at a level in
    /// dBFS, its query, or off. Mercury stops it by itself after 60 s.
    Tune(TuneArg),
}

/// The argument of `TUNE`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TuneArg {
    /// Key and hold the tone at this level, dBFS at the sound card (-60 to 0).
    Level(f32),
    /// `TUNE ?`: Mercury answers `TUNE <level>`, or WRONG if it has no TUNE.
    Query,
    Off,
}

// Levels are built in range (finite), so equality is total.
impl Eq for TuneArg {}

impl TuneArg {
    /// The tuning levels Mercury accepts, dBFS.
    pub const RANGE: (f32, f32) = (-60.0, 0.0);
}

impl Command {
    /// Wire form including the terminating CR.
    pub fn wire(&self) -> String {
        let on = |b: bool| if b { "ON" } else { "OFF" };
        let s = match self {
            Command::MyCall(c) => format!("MYCALL {c}"),
            Command::Listen(b) => format!("LISTEN {}", on(*b)),
            Command::Public(b) => format!("PUBLIC {}", on(*b)),
            Command::Bw(bw) => format!("BW{}", bw.token()),
            Command::Connect { mycall, theircall } => format!("CONNECT {mycall} {theircall}"),
            Command::Disconnect => "DISCONNECT".into(),
            Command::Abort => "ABORT".into(),
            Command::CqFrame { source, bw } => format!("CQFRAME {source} {}", bw.token()),
            Command::Mode(m) => format!("MODE {m}"),
            Command::BusyDetect(b) => format!("BUSYDETECT {}", on(*b)),
            Command::Monitor(b) => format!("MONITOR {}", on(*b)),
            Command::Tune(TuneArg::Level(db)) => format!("TUNE {db}"),
            Command::Tune(TuneArg::Query) => "TUNE ?".into(),
            Command::Tune(TuneArg::Off) => "TUNE OFF".into(),
        };
        s + "\r"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callsign_validation() {
        assert_eq!(Callsign::parse("zs6abc").unwrap().as_str(), "ZS6ABC");
        assert!(Callsign::parse("ZS6ABC/P").is_ok());
        assert!(Callsign::parse("N0CALL-1").is_ok());
        for bad in ["", "A B", "ABC\rLISTEN ON", "ABC\n", "ABCDEFGHIJKLMNOP", "--", "Ä1"] {
            assert!(Callsign::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn wire_forms() {
        let a = Callsign::parse("AAAA").unwrap();
        let b = Callsign::parse("BBBB").unwrap();
        assert_eq!(Command::Connect { mycall: a.clone(), theircall: b }.wire(), "CONNECT AAAA BBBB\r");
        assert_eq!(Command::Bw(Bandwidth::Bw500).wire(), "BW500\r");
        assert_eq!(Command::Listen(false).wire(), "LISTEN OFF\r");
        assert_eq!(Command::MyCall(a).wire(), "MYCALL AAAA\r");
        assert_eq!(Command::Mode(1).wire(), "MODE 1\r");
        assert_eq!(Command::BusyDetect(true).wire(), "BUSYDETECT ON\r");
    }
}
