//! Frequencies on a CAT line (M5 Task 11e): the four ways radios write them.
//!
//! A command has a `{}` where the frequency goes; the frequency is written as
//! `digits` characters and put there. Hex commands (`FEFE94E005{}FD`) carry
//! hex digits, so two characters are one byte; text commands (`FA{};`) carry
//! decimal digits.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreqCoding {
    /// Decimal digits as BCD, the least significant byte first (Icom CI-V).
    Bcd,
    /// Decimal digits as BCD, the most significant byte first.
    BcdBe,
    /// The number itself, most significant byte first (Ten-Tec Orion).
    Bin,
    /// Decimal digits as characters (Yaesu, Kenwood).
    Text,
}

impl FreqCoding {
    pub fn parse(s: &str) -> Option<FreqCoding> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "bcd" => FreqCoding::Bcd,
            "bcd-be" | "bcdbe" => FreqCoding::BcdBe,
            "bin" => FreqCoding::Bin,
            "text" => FreqCoding::Text,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            FreqCoding::Bcd => "bcd",
            FreqCoding::BcdBe => "bcd-be",
            FreqCoding::Bin => "bin",
            FreqCoding::Text => "text",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

fn err<T>(s: impl Into<String>) -> Result<T, CodecError> {
    Err(CodecError(s.into()))
}

/// `hz` as `digits` characters: the text to put where a command has `{}`.
/// `step_hz` is the radio's frequency unit (1, or 10 for some radios).
pub fn encode_freq(coding: FreqCoding, hz: u64, step_hz: u64, digits: usize) -> Result<String, CodecError> {
    if step_hz == 0 || digits == 0 || digits > 20 {
        return err("a frequency needs a step and 1 to 20 digits");
    }
    let n = hz / step_hz;
    match coding {
        FreqCoding::Text => {
            let s = format!("{n:0digits$}");
            if s.len() > digits {
                return err(format!("{hz} Hz does not fit {digits} digits"));
            }
            Ok(s)
        }
        FreqCoding::Bcd | FreqCoding::BcdBe => {
            let s = format!("{n:0digits$}");
            if s.len() > digits || !digits.is_multiple_of(2) {
                return err(format!("{hz} Hz does not fit {digits} BCD digits"));
            }
            if coding == FreqCoding::BcdBe {
                return Ok(s);
            }
            // Least significant byte first: reverse the pairs.
            let pairs: Vec<&str> = (0..s.len()).step_by(2).map(|i| &s[i..i + 2]).collect();
            Ok(pairs.into_iter().rev().collect())
        }
        FreqCoding::Bin => {
            if !digits.is_multiple_of(2) {
                return err("a binary frequency has a whole number of bytes");
            }
            let s = format!("{n:0digits$X}");
            if s.len() > digits {
                return err(format!("{hz} Hz does not fit {digits} hex digits"));
            }
            Ok(s)
        }
    }
}

/// The frequency in `chars` (the digits cut out of a reply), in Hz.
pub fn decode_freq(coding: FreqCoding, chars: &str, step_hz: u64) -> Result<u64, CodecError> {
    if step_hz == 0 || chars.is_empty() {
        return err("no frequency in the reply");
    }
    // Line noise can arrive as non-ASCII text; slicing it by bytes would panic.
    if !chars.is_ascii() {
        return err("the reply holds characters that are not a frequency");
    }
    let n = match coding {
        FreqCoding::Text => {
            chars.trim().parse::<u64>().map_err(|_| CodecError(format!("{chars:?} is not a frequency")))?
        }
        FreqCoding::Bcd | FreqCoding::BcdBe => {
            if !chars.len().is_multiple_of(2) {
                return err("a BCD frequency has a whole number of bytes");
            }
            let be: String = if coding == FreqCoding::Bcd {
                (0..chars.len()).step_by(2).map(|i| &chars[i..i + 2]).rev().collect()
            } else {
                chars.to_string()
            };
            if !be.bytes().all(|b| b.is_ascii_digit()) {
                return err(format!("{chars:?} is not BCD"));
            }
            be.parse::<u64>().map_err(|_| CodecError("a BCD frequency too long".into()))?
        }
        FreqCoding::Bin => u64::from_str_radix(chars, 16).map_err(|_| CodecError(format!("{chars:?} is not hex")))?,
    };
    n.checked_mul(step_hz).ok_or_else(|| CodecError("frequency out of range".into()))
}

/// A hex string as bytes ("FE 94" and "FE94" alike).
pub fn hex_to_bytes(s: &str) -> Result<Vec<u8>, CodecError> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !clean.len().is_multiple_of(2) || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return err(format!("{s:?} is not whole hex bytes"));
    }
    Ok((0..clean.len()).step_by(2).map(|i| u8::from_str_radix(&clean[i..i + 2], 16).expect("checked")).collect())
}

pub fn bytes_to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civ_bcd_puts_the_least_significant_byte_first() {
        // 14.074 MHz on an Icom: 00 40 07 14 00.
        assert_eq!(encode_freq(FreqCoding::Bcd, 14_074_000, 1, 10).unwrap(), "0040071400");
        assert_eq!(decode_freq(FreqCoding::Bcd, "0040071400", 1).unwrap(), 14_074_000);
    }

    #[test]
    fn text_is_zero_padded_decimal() {
        assert_eq!(encode_freq(FreqCoding::Text, 14_074_000, 1, 9).unwrap(), "014074000");
        assert_eq!(decode_freq(FreqCoding::Text, "014074000", 1).unwrap(), 14_074_000);
    }

    #[test]
    fn bcd_big_endian_in_ten_hertz_units() {
        // 7.105 MHz in 10 Hz units is 710500: 8 digits "00710500".
        assert_eq!(encode_freq(FreqCoding::BcdBe, 7_105_000, 10, 8).unwrap(), "00710500");
        assert_eq!(decode_freq(FreqCoding::BcdBe, "00710500", 10).unwrap(), 7_105_000);
    }

    #[test]
    fn binary_is_big_endian_hex() {
        // Ten-Tec: 14.074 MHz is 0xD6C090 in four bytes.
        assert_eq!(encode_freq(FreqCoding::Bin, 14_074_000, 1, 8).unwrap(), "00D6C090");
        assert_eq!(decode_freq(FreqCoding::Bin, "00D6C090", 1).unwrap(), 14_074_000);
    }

    #[test]
    fn a_frequency_that_does_not_fit_is_refused_not_cut() {
        assert!(encode_freq(FreqCoding::Text, 1_234_567_890, 1, 9).is_err());
        assert!(encode_freq(FreqCoding::Bcd, 1_234_567_890_123, 1, 10).is_err());
    }

    #[test]
    fn garbage_in_a_reply_is_an_error() {
        assert!(decode_freq(FreqCoding::Bcd, "00A0071400", 1).is_err());
        assert!(decode_freq(FreqCoding::Text, "FA0140", 1).is_err());
        assert!(decode_freq(FreqCoding::Bcd, "004", 1).is_err());
    }

    #[test]
    fn non_ascii_in_a_reply_is_an_error_not_a_panic() {
        // Line noise turned into the replacement character by a lossy text conversion.
        assert!(decode_freq(FreqCoding::Bcd, "00\u{FFFD}071400", 1).is_err());
        assert!(decode_freq(FreqCoding::Bcd, "\u{FFFD}\u{FFFD}", 1).is_err());
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex_to_bytes("FE FE 94 E0").unwrap(), vec![0xFE, 0xFE, 0x94, 0xE0]);
        assert_eq!(bytes_to_hex(&[0xFE, 0x0D]), "FE0D");
        assert!(hex_to_bytes("FEF").is_err());
    }
}
