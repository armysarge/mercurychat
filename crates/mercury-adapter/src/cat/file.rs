//! The CAT commands file (M5 Task 11e): one section per radio, `key = value`
//! lines, `#` comments. Commands are hex bytes (`FEFE94E01C0001FD`) or text
//! (`TX1;`); several separated by commas are sent in order.
//!
//! ```text
//! [Icom IC-7300]
//! encoding        = hex
//! ptt_on          = FEFE94E01C0001FD
//! ptt_off         = FEFE94E01C0000FD
//! mode_usb_data   = FEFE94E02600010102FD
//! set_freq        = FEFE94E005{}FD
//! freq_coding     = bcd
//! freq_digits     = 10
//! read_freq       = FEFE94E003FD
//! read_len        = 22        # characters of a reply, the echo not counted
//! read_start      = 10        # where the frequency starts (0 = the first)
//! read_digits     = 10
//! ```
//!
//! A file in the common table layout (`CmdType`, `PTTOn`, `SetFreqVfoA_Cmd`,
//! ...) is read by [`import`] and kept in this one.

use super::codec::{bytes_to_hex, decode_freq, encode_freq, hex_to_bytes, FreqCoding};
use std::collections::BTreeMap;
use std::fmt;

/// The radios that ship with the app, written from the makers' CAT manuals.
pub const BUILTIN: &str = include_str!("builtin.mccat");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError(pub String);

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FileError {}

fn bad<T>(s: impl Into<String>) -> Result<T, FileError> {
    Err(FileError(s.into()))
}

/// The rig modes a file can set, as rigctld names them.
pub const MODES: [(&str, &str); 8] = [
    ("USB", "mode_usb"),
    ("PKTUSB", "mode_usb_data"),
    ("LSB", "mode_lsb"),
    ("PKTLSB", "mode_lsb_data"),
    ("FM", "mode_fm"),
    ("PKTFM", "mode_fm_data"),
    ("AM", "mode_am"),
    ("CW", "mode_cw"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Radio {
    pub name: String,
    /// Commands are text rather than hex bytes.
    pub text: bool,
    pub ptt_on: Vec<Vec<u8>>,
    pub ptt_off: Vec<Vec<u8>>,
    /// rigctld mode name -> commands.
    pub modes: BTreeMap<String, Vec<Vec<u8>>>,
    /// The set-frequency command with `{}` where the frequency goes.
    pub set_freq: String,
    pub freq_coding: FreqCoding,
    pub freq_digits: usize,
    /// The radio's frequency unit in Hz (1, or 10).
    pub freq_step_hz: u64,
    pub read_freq: Vec<u8>,
    pub read_coding: FreqCoding,
    pub read_step_hz: u64,
    /// Characters in a reply (hex digits or text), the echo not counted.
    pub read_len: usize,
    /// Where the frequency starts in it (0 = the first character).
    pub read_start: usize,
    pub read_digits: usize,
    /// Sent when the engine starts and stops (set up the rig, restore it).
    pub start: Vec<Vec<u8>>,
    pub exit: Vec<Vec<u8>>,
}

impl Radio {
    /// The bytes that set `hz`.
    pub fn set_freq_bytes(&self, hz: u64) -> Result<Vec<u8>, FileError> {
        let digits =
            encode_freq(self.freq_coding, hz, self.freq_step_hz, self.freq_digits).map_err(|e| FileError(e.0))?;
        let line = self.set_freq.replacen("{}", &digits, 1);
        self.command_bytes(&line)
    }

    /// The frequency in a reply to `read_freq` (anything before the last
    /// `read_len` characters, such as an echo, is ignored).
    pub fn parse_freq_reply(&self, reply: &[u8]) -> Result<u64, FileError> {
        let chars = if self.text { String::from_utf8_lossy(reply).into_owned() } else { bytes_to_hex(reply) };
        let chars: Vec<char> = chars.chars().collect();
        if chars.len() < self.read_len {
            return bad(format!("the reply is {} characters, expected {}", chars.len(), self.read_len));
        }
        let tail = &chars[chars.len() - self.read_len..];
        let end = self.read_start + self.read_digits;
        if end > tail.len() {
            return bad("the frequency lies outside the reply");
        }
        let digits: String = tail[self.read_start..end].iter().collect();
        decode_freq(self.read_coding, &digits, self.read_step_hz).map_err(|e| FileError(e.0))
    }

    fn command_bytes(&self, s: &str) -> Result<Vec<u8>, FileError> {
        if self.text {
            Ok(s.as_bytes().to_vec())
        } else {
            hex_to_bytes(s).map_err(|e| FileError(e.0))
        }
    }

    /// The radio in this file's own form.
    pub fn to_text(&self) -> String {
        let show = |cmds: &[Vec<u8>]| -> String {
            cmds.iter()
                .map(|c| if self.text { String::from_utf8_lossy(c).into_owned() } else { bytes_to_hex(c) })
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut o = format!("[{}]\nencoding = {}\n", self.name, if self.text { "text" } else { "hex" });
        let mut kv = |k: &str, v: String| {
            if !v.is_empty() {
                o.push_str(&format!("{k} = {v}\n"));
            }
        };
        kv("ptt_on", show(&self.ptt_on));
        kv("ptt_off", show(&self.ptt_off));
        for (mode, key) in MODES {
            if let Some(c) = self.modes.get(mode) {
                kv(key, show(c));
            }
        }
        kv("set_freq", self.set_freq.clone());
        kv("freq_coding", self.freq_coding.name().to_string());
        kv("freq_digits", self.freq_digits.to_string());
        kv("freq_step_hz", self.freq_step_hz.to_string());
        kv(
            "read_freq",
            if self.text {
                String::from_utf8_lossy(&self.read_freq).into_owned()
            } else {
                bytes_to_hex(&self.read_freq)
            },
        );
        kv("read_coding", self.read_coding.name().to_string());
        kv("read_step_hz", self.read_step_hz.to_string());
        kv("read_len", self.read_len.to_string());
        kv("read_start", self.read_start.to_string());
        kv("read_digits", self.read_digits.to_string());
        kv("start", show(&self.start));
        kv("exit", show(&self.exit));
        o.push('\n');
        o
    }
}

/// What a file held: the radios that read, and each one that did not, with why.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    pub radios: Vec<Radio>,
    pub skipped: Vec<(String, String)>,
}

/// `[section]` blocks of `key = value`, in order. A `#` at the start of a
/// line is a comment; `;` is not (text commands end in it).
fn sections(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut out: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if let Some(name) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            out.push((name.trim().to_string(), Vec::new()));
        } else if let (Some((k, v)), Some(last)) = (t.split_once('='), out.last_mut()) {
            // A trailing "  # note" after a value is a comment too.
            let v = v.split(" #").next().unwrap_or(v);
            last.1.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

/// Commands from a value: comma-separated, unless it starts `[ALLOW_COMMA]`.
fn commands(value: &str, text: bool) -> Result<Vec<Vec<u8>>, FileError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(vec![]);
    }
    let (value, split) = match value.strip_prefix("[ALLOW_COMMA]") {
        Some(rest) => (rest.trim(), false),
        None => (value, true),
    };
    let parts: Vec<&str> = if split { value.split(',').collect() } else { vec![value] };
    parts
        .into_iter()
        .map(|p| {
            let p = p.trim();
            if text {
                Ok(p.as_bytes().to_vec())
            } else {
                hex_to_bytes(p).map_err(|e| FileError(e.0))
            }
        })
        .collect()
}

fn build(name: &str, kv: &[(String, String)]) -> Result<Radio, FileError> {
    let get = |k: &str| kv.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
    let num = |k: &str, default: Option<usize>| -> Result<usize, FileError> {
        match (get(k), default) {
            (Some(v), _) => v.trim().parse().map_err(|_| FileError(format!("{k} is not a number: {v:?}"))),
            (None, Some(d)) => Ok(d),
            (None, None) => bad(format!("{k} is missing")),
        }
    };
    let text = match get("encoding").map(|e| e.to_ascii_lowercase()) {
        Some(e) if e == "text" => true,
        Some(e) if e == "hex" => false,
        Some(e) => return bad(format!("encoding must be hex or text, not {e:?}")),
        None => return bad("encoding is missing (hex or text)"),
    };
    let coding = |k: &str| -> Result<FreqCoding, FileError> {
        let v = get(k).ok_or_else(|| FileError(format!("{k} is missing")))?;
        FreqCoding::parse(v).ok_or_else(|| FileError(format!("{k}: {v:?} is not bcd, bcd-be, bin or text")))
    };
    let cmds = |k: &str| commands(get(k).unwrap_or(""), text);
    let mut modes = BTreeMap::new();
    for (mode, key) in MODES {
        let c = cmds(key)?;
        if !c.is_empty() {
            modes.insert(mode.to_string(), c);
        }
    }
    let set_freq = get("set_freq").ok_or_else(|| FileError("set_freq is missing".into()))?.to_string();
    if !set_freq.contains("{}") {
        return bad("set_freq has no {} for the frequency");
    }
    let read_freq = {
        let v = get("read_freq").ok_or_else(|| FileError("read_freq is missing".into()))?;
        if text {
            v.as_bytes().to_vec()
        } else {
            hex_to_bytes(v).map_err(|e| FileError(e.0))?
        }
    };
    let radio = Radio {
        name: name.to_string(),
        text,
        ptt_on: cmds("ptt_on")?,
        ptt_off: cmds("ptt_off")?,
        modes,
        set_freq,
        freq_coding: coding("freq_coding")?,
        freq_digits: num("freq_digits", None)?,
        freq_step_hz: num("freq_step_hz", Some(1))? as u64,
        read_freq,
        read_coding: coding("read_coding").or_else(|_| coding("freq_coding"))?,
        read_step_hz: num("read_step_hz", Some(num("freq_step_hz", Some(1))?))? as u64,
        read_len: num("read_len", None)?,
        read_start: num("read_start", None)?,
        read_digits: num("read_digits", None)?,
        start: cmds("start")?,
        exit: cmds("exit")?,
    };
    // A radio that cannot set a frequency is no radio: try one.
    radio.set_freq_bytes(14_074_000)?;
    Ok(radio)
}

/// Read a file in this format.
pub fn parse(text: &str) -> Loaded {
    let mut out = Loaded::default();
    for (name, kv) in sections(text) {
        match build(&name, &kv) {
            Ok(r) => out.radios.push(r),
            Err(e) => out.skipped.push((name, e.0)),
        }
    }
    out
}

/// A file in the common table layout (`CmdType`, `PTTOn`, `ModeUSB_D`,
/// `SetFreqVfoA_Cmd`, `ReadFreqVfoA_Result_*`): each radio becomes one in this
/// format. Positions there count from 1; here from 0.
pub fn import(text: &str) -> Loaded {
    let mut out = Loaded::default();
    for (name, kv) in sections(text) {
        let get = |k: &str| kv.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone());
        let mut m: Vec<(String, String)> = Vec::new();
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v.filter(|v| !v.is_empty()) {
                m.push((k.to_string(), v));
            }
        };
        let coding = |v: Option<String>| v.map(|v| v.to_ascii_lowercase().replace("bcdbe", "bcd-be"));
        put("encoding", get("CmdType").map(|v| v.to_ascii_lowercase()));
        put("ptt_on", get("PTTOn"));
        put("ptt_off", get("PTTOff"));
        put("mode_usb", get("ModeUSB"));
        put("mode_usb_data", get("ModeUSB_D"));
        put("mode_fm", get("ModeFM"));
        put("mode_fm_data", get("ModeFM_D"));
        put("set_freq", get("SetFreqVfoA_Cmd"));
        put("freq_coding", coding(get("SetFreq_Data_method")));
        put("freq_digits", get("SetFreqVfoA_param_length"));
        put("freq_step_hz", get("SetFreqVfoA_hz_res"));
        put("read_freq", get("ReadFreqVfoA_Cmd"));
        put("read_coding", coding(get("ReadFreqVfoA_Result_Data_method")));
        put("read_step_hz", get("ReadFreqVfoA_Result_hz_res"));
        put("read_len", get("ReadFreqVfoA_Result_Length"));
        put(
            "read_start",
            get("ReadFreqVfoA_Result_Freq_Start_Pos")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .map(|p| p.saturating_sub(1).to_string()),
        );
        put("read_digits", get("ReadFreqVfoA_Result_Freq_Length"));
        put("start", get("VarACStartCmd"));
        put("exit", get("VarACExitCmd"));
        match build(&name, &m) {
            Ok(r) => out.radios.push(r),
            Err(e) => out.skipped.push((name, e.0)),
        }
    }
    out
}

/// Whichever layout `text` is in.
pub fn load(text: &str) -> Loaded {
    if text.contains("\nset_freq") || text.starts_with("set_freq") || text.contains("\nencoding") {
        parse(text)
    } else {
        import(text)
    }
}

/// The built-in radios.
pub fn builtin() -> Vec<Radio> {
    let l = parse(BUILTIN);
    debug_assert!(l.skipped.is_empty(), "built-in radios must all read: {:?}", l.skipped);
    l.radios
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICOM: &str = "\
[Icom IC-7300]
CmdType=HEX
PTTOn=FEFE94E01C0001FD
PTTOff=FEFE94E01C0000FD
ModeUSB=FEFE94E026000100FD
ModeUSB_D=FEFE94E02600010102FD
SetFreq_Data_method=BCD
SetFreqVfoA_Cmd=FEFE94E02500{}FD
SetFreqVfoA_param_length=10
SetFreqVfoA_hz_res=1
ReadFreqVfoA_Cmd=FEFE94E02500FD
ReadFreqVfoA_Result_Data_method=BCD
ReadFreqVfoA_Result_hz_res=1
ReadFreqVfoA_Result_Length=24
ReadFreqVfoA_Result_Freq_Start_Pos=13
ReadFreqVfoA_Result_Freq_Length=10
VarACStartCmd=
VarACExitCmd=

[Yaesu FT-991A]
CmdType=TEXT
PTTOn=TX1;
PTTOff=TX0;
ModeUSB_D=MD0C;
SetFreq_Data_method=TEXT
SetFreqVfoA_Cmd=FA{};
SetFreqVfoA_param_length=9
SetFreqVfoA_hz_res=1
ReadFreqVfoA_Cmd=FA;
ReadFreqVfoA_Result_Data_method=TEXT
ReadFreqVfoA_Result_hz_res=1
ReadFreqVfoA_Result_Length=12
ReadFreqVfoA_Result_Freq_Start_Pos=3
ReadFreqVfoA_Result_Freq_Length=9

[Broken]
CmdType=HEX
PTTOn=FEF
";

    #[test]
    fn a_table_file_is_imported_and_the_broken_radio_is_named() {
        let l = import(ICOM);
        assert_eq!(l.radios.len(), 2);
        assert_eq!(l.skipped.len(), 1);
        assert_eq!(l.skipped[0].0, "Broken");
        let icom = &l.radios[0];
        assert_eq!(icom.name, "Icom IC-7300");
        assert_eq!(icom.read_start, 12, "positions count from 0 here");
        assert_eq!(bytes_to_hex(&icom.set_freq_bytes(14_074_000).unwrap()), "FEFE94E025000040071400FD");
        assert_eq!(bytes_to_hex(&icom.ptt_on[0]), "FEFE94E01C0001FD");
        assert!(icom.modes.contains_key("PKTUSB") && icom.modes.contains_key("USB"));
        let yaesu = &l.radios[1];
        assert_eq!(yaesu.set_freq_bytes(7_105_000).unwrap(), b"FA007105000;".to_vec());
    }

    #[test]
    fn a_reply_is_read_with_or_without_an_echo() {
        let icom = &import(ICOM).radios[0];
        // FEFEE094 2500 + 5 bytes + FD.
        let reply = hex_to_bytes("FEFEE09425000040071400FD").unwrap();
        assert_eq!(icom.parse_freq_reply(&reply).unwrap(), 14_074_000);
        let mut echoed = hex_to_bytes("FEFE94E02500FD").unwrap();
        echoed.extend(&reply);
        assert_eq!(icom.parse_freq_reply(&echoed).unwrap(), 14_074_000, "the echo before it is ignored");
        assert!(icom.parse_freq_reply(&reply[..6]).is_err(), "a short reply is an error");
        let yaesu = &import(ICOM).radios[1];
        assert_eq!(yaesu.parse_freq_reply(b"FA014074000;").unwrap(), 14_074_000);
    }

    #[test]
    fn our_format_round_trips_an_imported_radio() {
        let first = import(ICOM);
        let ours: String = first.radios.iter().map(Radio::to_text).collect();
        let again = parse(&ours);
        assert!(again.skipped.is_empty(), "{:?}", again.skipped);
        assert_eq!(again.radios, first.radios);
        assert!(!ours.contains("CmdType"), "the file is in our own form");
    }

    #[test]
    fn load_tells_the_layouts_apart() {
        let ours: String = import(ICOM).radios.iter().map(Radio::to_text).collect();
        assert_eq!(load(&ours).radios.len(), 2);
        assert_eq!(load(ICOM).radios.len(), 2);
    }

    #[test]
    fn commas_make_several_commands_unless_allowed() {
        assert_eq!(commands("FE94,FD01", false).unwrap().len(), 2);
        assert_eq!(commands("[ALLOW_COMMA]MD0,2;", true).unwrap(), vec![b"MD0,2;".to_vec()]);
        assert_eq!(commands("", false).unwrap(), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn a_radio_must_say_how_to_set_a_frequency() {
        let l = parse("[No freq]\nencoding = hex\nptt_on = FE\n");
        assert!(l.radios.is_empty());
        assert!(l.skipped[0].1.contains("set_freq"), "{:?}", l.skipped);
    }

    /// The radios that ship with the app all read, and each one sets 14.074 MHz.
    #[test]
    fn the_built_in_radios_all_read() {
        let l = parse(BUILTIN);
        assert!(l.skipped.is_empty(), "{:?}", l.skipped);
        assert!(l.radios.iter().any(|r| r.name == "Icom IC-7300"));
        for r in &l.radios {
            assert!(!r.set_freq_bytes(14_074_000).unwrap().is_empty(), "{}", r.name);
            assert!(!r.ptt_on.is_empty() && !r.ptt_off.is_empty(), "{} has no PTT", r.name);
        }
    }
}
