//! Hamlib rigctld's network protocol, extended-response form (docs/02 §5.1).
//!
//! Every request is sent with a `+` prefix, so every reply is a block that
//! repeats the command name, gives `Key: value` lines, and ends with
//! `RPRT n` (0 is success). That last line makes a reply unambiguous to
//! frame and parse, whatever the rig.
//!
//! MercuryChat never keys the transmitter: Mercury does (02 §5.1). So there
//! is deliberately no request that sets PTT on; [`RigRequest::PttOff`] is the
//! only PTT write, used once after a lost rig link (M3b design §7.9).

/// A rig level MercuryChat reads or sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    /// RF power setting, 0.0–1.0 of the rig's maximum.
    RfPower,
    /// SWR meter while transmitting.
    Swr,
    /// ALC meter while transmitting.
    Alc,
    /// Power-out meter while transmitting, 0.0–1.0.
    RfPowerMeter,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::RfPower, Level::Swr, Level::Alc, Level::RfPowerMeter];

    pub fn token(&self) -> &'static str {
        match self {
            Level::RfPower => "RFPOWER",
            Level::Swr => "SWR",
            Level::Alc => "ALC",
            Level::RfPowerMeter => "RFPOWER_METER",
        }
    }

    fn from_token(t: &str) -> Option<Level> {
        Level::ALL.into_iter().find(|l| l.token() == t)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RigRequest {
    GetFreq,
    SetFreq(u64),
    GetMode,
    SetMode {
        mode: String,
        passband_hz: i32,
    },
    GetVfo,
    SetVfo(String),
    GetSplit,
    SetSplit {
        on: bool,
        tx_vfo: String,
    },
    GetPtt,
    /// `T 0`. There is no PTT-on request, by design.
    PttOff,
    GetLevel(Level),
    SetLevel(Level, f32),
    DumpCaps,
}

impl RigRequest {
    /// The request as sent, extended form, newline-terminated.
    pub fn wire(&self) -> String {
        let body = match self {
            RigRequest::GetFreq => "f".to_string(),
            RigRequest::SetFreq(hz) => format!("F {hz}"),
            RigRequest::GetMode => "m".to_string(),
            RigRequest::SetMode { mode, passband_hz } => format!("M {mode} {passband_hz}"),
            RigRequest::GetVfo => "v".to_string(),
            RigRequest::SetVfo(v) => format!("V {v}"),
            RigRequest::GetSplit => "s".to_string(),
            RigRequest::SetSplit { on, tx_vfo } => format!("S {} {tx_vfo}", u8::from(*on)),
            RigRequest::GetPtt => "t".to_string(),
            RigRequest::PttOff => "T 0".to_string(),
            RigRequest::GetLevel(l) => format!("l {}", l.token()),
            RigRequest::SetLevel(l, v) => format!("L {} {v}", l.token()),
            RigRequest::DumpCaps => "\\dump_caps".to_string(),
        };
        format!("+{body}\n")
    }
}

/// What the rig said it can do, from `\dump_caps`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RigCaps {
    pub model: String,
    pub can_set_freq: bool,
    pub can_set_mode: bool,
    pub can_set_vfo: bool,
    pub can_split: bool,
    pub can_get_ptt: bool,
    pub get_levels: Vec<Level>,
    pub set_levels: Vec<Level>,
    pub modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RigReply {
    Freq(u64),
    Mode {
        mode: String,
        passband_hz: i32,
    },
    Vfo(String),
    Split {
        on: bool,
        tx_vfo: String,
    },
    Ptt(bool),
    Level(f32),
    Caps(RigCaps),
    /// A set request succeeded.
    Ok,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigProtoError {
    /// rigctld answered `RPRT n` with n ≠ 0.
    Rprt(i32),
    Malformed(String),
    /// No `RPRT` line yet.
    Incomplete,
}

fn rprt(line: &str) -> Option<i32> {
    line.strip_prefix("RPRT ")?.trim().parse().ok()
}

/// True once the buffer holds a whole reply block: a complete `RPRT n` line.
pub fn block_complete(buf: &str) -> bool {
    buf.split_inclusive('\n').any(|l| l.ends_with('\n') && rprt(l.trim_end_matches(['\r', '\n'])).is_some())
}

/// Parse one reply block for the request that produced it.
pub fn parse_reply(req: &RigRequest, block: &str) -> Result<RigReply, RigProtoError> {
    if !block_complete(block) {
        return Err(RigProtoError::Incomplete);
    }
    let lines: Vec<&str> = block.lines().map(|l| l.trim_end_matches('\r')).collect();
    let code = lines.iter().find_map(|l| rprt(l)).unwrap_or(0);
    if code != 0 {
        return Err(RigProtoError::Rprt(code));
    }
    let bad = |what: &str| RigProtoError::Malformed(format!("{what} in {:?}", block.lines().next().unwrap_or("")));
    let value = |key: &str| -> Option<&str> {
        lines.iter().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')).map(str::trim))
    };
    match req {
        RigRequest::GetFreq => {
            let f = value("Frequency").ok_or_else(|| bad("no Frequency"))?;
            // rigctld prints a float for some backends ("7105000.000000").
            let hz = f
                .parse::<u64>()
                .ok()
                .or_else(|| f.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v.round() as u64));
            hz.map(RigReply::Freq).ok_or_else(|| bad("bad Frequency"))
        }
        RigRequest::GetMode => {
            let mode = value("Mode").filter(|m| !m.is_empty()).ok_or_else(|| bad("no Mode"))?;
            let pb = value("Passband").and_then(|p| p.parse().ok()).ok_or_else(|| bad("bad Passband"))?;
            Ok(RigReply::Mode { mode: mode.to_string(), passband_hz: pb })
        }
        RigRequest::GetVfo => {
            value("VFO").filter(|v| !v.is_empty()).map(|v| RigReply::Vfo(v.into())).ok_or_else(|| bad("no VFO"))
        }
        RigRequest::GetSplit => {
            let on = match value("Split") {
                Some("0") => false,
                Some("1") => true,
                _ => return Err(bad("bad Split")),
            };
            let tx_vfo = value("TX VFO").ok_or_else(|| bad("no TX VFO"))?;
            Ok(RigReply::Split { on, tx_vfo: tx_vfo.to_string() })
        }
        RigRequest::GetPtt => match value("PTT") {
            Some("0") => Ok(RigReply::Ptt(false)),
            // 1 is PTT on; 2 and 3 are on via the mic or data jack.
            Some("1" | "2" | "3") => Ok(RigReply::Ptt(true)),
            _ => Err(bad("bad PTT")),
        },
        RigRequest::GetLevel(_) => {
            // `get_level: RFPOWER`, then the bare value.
            let v = lines.get(1).filter(|l| rprt(l).is_none()).ok_or_else(|| bad("no level value"))?;
            v.trim().parse::<f32>().ok().filter(|v| v.is_finite()).map(RigReply::Level).ok_or_else(|| bad("bad level"))
        }
        RigRequest::DumpCaps => Ok(RigReply::Caps(parse_caps(&lines))),
        RigRequest::SetFreq(_)
        | RigRequest::SetMode { .. }
        | RigRequest::SetVfo(_)
        | RigRequest::SetSplit { .. }
        | RigRequest::PttOff
        | RigRequest::SetLevel(..) => Ok(RigReply::Ok),
    }
}

fn parse_caps(lines: &[&str]) -> RigCaps {
    let field =
        |key: &str| lines.iter().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')).map(str::trim));
    let yes = |key: &str| field(key).is_some_and(|v| v.starts_with('Y'));
    // "Get level: PREAMP(0..0/0) RFPOWER(0.05..1/0.002) ..."
    let levels = |key: &str| -> Vec<Level> {
        field(key)
            .unwrap_or("")
            .split_whitespace()
            .filter_map(|t| Level::from_token(t.split('(').next().unwrap_or("")))
            .collect()
    };
    RigCaps {
        model: field("Model name").unwrap_or("").to_string(),
        can_set_freq: yes("Can set Frequency"),
        can_set_mode: yes("Can set Mode"),
        can_set_vfo: yes("Can set VFO"),
        can_split: yes("Can set Split VFO") || yes("Can set Split Freq"),
        can_get_ptt: yes("Can get PTT"),
        get_levels: levels("Get level"),
        set_levels: levels("Set level"),
        modes: field("Mode list").unwrap_or("").split_whitespace().map(String::from).collect(),
    }
}
