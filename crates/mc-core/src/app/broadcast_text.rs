//! The text of a general broadcast, `CALL: message`: the form Mercury itself
//! reads a sender from. Composing it for the Broadcast button, and reading
//! other stations' (untrusted text from the air).

use mercury_adapter::kiss::wrapped_capacity;
use mercury_adapter::Callsign;

/// The longest message the Broadcast button sends, in bytes.
pub const MESSAGE_MAX: usize = 40;

/// Between the callsign and the message.
const SEPARATOR: &str = ": ";

/// A callsign as the list accepts one. `Callsign::parse` also takes words
/// such as `NOTE`, so a letter and a digit are required.
fn callsign(s: &str) -> Option<Callsign> {
    let call = Callsign::parse(s).ok()?;
    let has = |f: fn(&u8) -> bool| call.as_str().bytes().any(|b| f(&b));
    (has(u8::is_ascii_alphabetic) && has(u8::is_ascii_digit)).then_some(call)
}

/// One line of plain text: no control characters, and none of the invisible
/// ones that reorder or hide what is shown.
fn plain(s: &str) -> bool {
    !s.chars().any(|c| {
        c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
}

/// The payload for `text` from `call` on `listen_mode`: never cut, so a
/// message that does not fit is refused with the reason.
pub fn compose(call: &str, text: &str, listen_mode: u8) -> Result<Vec<u8>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("not sent: type a message first".into());
    }
    if text.len() > MESSAGE_MAX {
        return Err(format!("not sent: a broadcast message is up to {MESSAGE_MAX} bytes"));
    }
    if !plain(text) {
        return Err("not sent: a broadcast message is one line of plain text".into());
    }
    if callsign(call).is_none() {
        return Err("not sent: other stations list a broadcast only from a callsign with a letter and a digit".into());
    }
    let Some(capacity) = wrapped_capacity(listen_mode) else {
        return Err("not sent: listen mode unknown".into());
    };
    let payload = format!("{call}{SEPARATOR}{text}").into_bytes();
    if payload.len() > capacity {
        return Err(format!(
            "not sent: with your callsign it is {} bytes, over the {capacity}-byte limit of this listen mode",
            payload.len()
        ));
    }
    Ok(payload)
}

/// (callsign, message) if `payload` is `CALL: message`. The callsign is what
/// the text claims: nothing proves who sent it. Another program's padding and
/// line ending are trimmed.
pub fn parse(payload: &[u8]) -> Option<(String, String)> {
    let end = payload.iter().rposition(|b| !matches!(b, 0 | b'\r' | b'\n'))? + 1;
    let (call, message) = std::str::from_utf8(&payload[..end]).ok()?.split_once(SEPARATOR)?;
    // Exactly the callsign before the colon: parse() would trim spaces.
    let call = (call == call.trim()).then(|| callsign(call))??;
    let message = message.trim();
    (!message.is_empty() && plain(message)).then(|| (call.as_str().to_string(), message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATAC3: u8 = 1;
    const DATAC15: u8 = 7;

    #[test]
    fn composes_call_colon_message() {
        assert_eq!(compose("ZS6ABC", "  QRV 40m ", DATAC3).unwrap(), b"ZS6ABC: QRV 40m");
    }

    #[test]
    fn compose_refuses_empty_long_multiline() {
        for bad in ["", "   ", &"x".repeat(MESSAGE_MAX + 1), "a\nb", "a\u{202e}b"] {
            let why = compose("ZS6ABC", bad, DATAC3).unwrap_err();
            assert!(why.starts_with("not sent: "), "{bad:?}: {why}");
        }
        assert!(compose("ZS6ABC", &"x".repeat(MESSAGE_MAX), DATAC3).is_ok());
    }

    #[test]
    fn compose_refuses_over_the_modes_capacity() {
        // DATAC15 carries 27 bytes: "ZS6ABC: " and 19 more fit, 20 do not.
        assert!(compose("ZS6ABC", &"x".repeat(19), DATAC15).is_ok());
        let why = compose("ZS6ABC", &"x".repeat(20), DATAC15).unwrap_err();
        assert!(why.starts_with("not sent: ") && why.contains("27"), "{why}");
        let unknown = compose("ZS6ABC", "hi", 200).unwrap_err();
        assert!(unknown.contains("listen mode"), "{unknown}");
    }

    #[test]
    fn compose_refuses_a_callsign_nobody_would_list() {
        let why = compose("LABA", "hi", DATAC3).unwrap_err();
        assert!(why.starts_with("not sent: "), "{why}");
    }

    fn read(s: &[u8]) -> Option<(String, String)> {
        parse(s)
    }

    fn got(call: &str, message: &str) -> Option<(String, String)> {
        Some((call.to_string(), message.to_string()))
    }

    #[test]
    fn parses_what_it_composes() {
        let payload = compose("ZS6ABC/P", "net at 18:00: 7.105", DATAC3).unwrap();
        assert_eq!(read(&payload), got("ZS6ABC/P", "net at 18:00: 7.105"));
    }

    #[test]
    fn reads_a_long_message_from_another_program() {
        let payload = format!("zs1xyz: {}", "é".repeat(100));
        assert_eq!(read(payload.as_bytes()), got("ZS1XYZ", &"é".repeat(100)));
    }

    #[test]
    fn trims_padding_and_line_ends() {
        assert_eq!(read(b"ZS1XYZ: hi\r\n\0\0"), got("ZS1XYZ", "hi"));
    }

    #[test]
    fn rejects_lookalikes() {
        for bad in [
            "NOTE: net tonight",
            "RE: antenna",
            "73: all",
            "ZS6ABC:no space",
            "ZS6ABC: ",
            "ZS6ABC",
            ": x",
            "ZS6 ABC: x",
            "AAAAAAAAAAAAAAA1: too long a callsign",
        ] {
            assert_eq!(read(bad.as_bytes()), None, "{bad:?}");
        }
    }

    #[test]
    fn rejects_hostile_text() {
        for bad in [
            &b"ZS6ABC: \x1b[31mred"[..],
            b"ZS6ABC: one\ntwo",
            b"ZS6ABC: nul\0inside",
            "ZS6ABC: \u{202e}reversed".as_bytes(),
            "ZS6ABC: zero\u{200b}width".as_bytes(),
            "ZS6ABC: \u{2066}isolate".as_bytes(),
            b"ZS6ABC: \xff\xfe",
        ] {
            assert_eq!(read(bad), None, "{bad:?}");
        }
    }
}
