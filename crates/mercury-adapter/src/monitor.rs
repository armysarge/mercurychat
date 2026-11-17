//! Parsing of Mercury's `MONITOR` lines (docs/03 §6).

/// A CQ heard by a monitoring station.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorCq {
    pub from: String,
    pub bw: String,
    pub snr_db: f32,
}

/// Parse the text after `MONITOR `: `DATAC16 CQ FROM=<call> BW=<bw> SNR=<dB>`.
/// Anything else is `None`. The callsign is not validated here; it is
/// untrusted text from the air and the caller must run `Callsign::parse`.
pub fn parse_cq(rest: &str) -> Option<MonitorCq> {
    let mut it = rest.split_whitespace();
    if it.next()? != "DATAC16" || it.next()? != "CQ" {
        return None;
    }
    let from = it.next()?.strip_prefix("FROM=")?;
    let bw = it.next()?.strip_prefix("BW=")?;
    let snr = it.next()?.strip_prefix("SNR=")?;
    if it.next().is_some() || from.is_empty() || bw.is_empty() {
        return None;
    }
    let snr_db: f32 = snr.parse().ok()?;
    if !snr_db.is_finite() {
        return None;
    }
    Some(MonitorCq { from: from.to_string(), bw: bw.to_string(), snr_db })
}
