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

/// A broadcast frame a monitoring station decoded: the mode and the S/N. It
/// names no sender.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorBroadcast {
    pub mode: String,
    pub snr_db: f32,
}

/// Parse the text after `MONITOR `: `<mode> BCAST_DATA LEN=<n> SNR=<dB>`
/// (Mercury `docs/TNC.md`, "MONITOR"). Anything else is `None`. `LEN` is the
/// whole modem frame, the same for every broadcast on a mode, so it is
/// checked and dropped.
pub fn parse_broadcast(rest: &str) -> Option<MonitorBroadcast> {
    let mut it = rest.split_whitespace();
    let mode = it.next()?;
    if mode.len() > 8 || !mode.bytes().all(|b| b.is_ascii_alphanumeric()) || it.next()? != "BCAST_DATA" {
        return None;
    }
    it.next()?.strip_prefix("LEN=")?.parse::<usize>().ok()?;
    let snr_db: f32 = it.next()?.strip_prefix("SNR=")?.parse().ok()?;
    if it.next().is_some() || !snr_db.is_finite() {
        return None;
    }
    Some(MonitorBroadcast { mode: mode.to_string(), snr_db })
}

#[cfg(test)]
mod broadcast_report_tests {
    use super::*;

    #[test]
    fn parses_a_broadcast_report() {
        let r = parse_broadcast("DATAC3 BCAST_DATA LEN=124 SNR=7.5").unwrap();
        assert_eq!((r.mode.as_str(), r.snr_db), ("DATAC3", 7.5));
    }

    #[test]
    fn other_monitor_lines_are_not_broadcast_reports() {
        for line in [
            "DATAC3 BCAST_CTRL LEN=124 SNR=7.5",
            "DATAC16 STATION LAYOUT=1 SNR=7.5",
            "DATAC16 CQ FROM=X BW=500 SNR=7.5",
            "DATAC3 BCAST_DATA LEN=124 SNR=nan",
            "DATAC3 BCAST_DATA LEN=124 SNR=7.5 EXTRA",
            "DATAC3 BCAST_DATA LEN=x SNR=7.5",
            "DATAC3 BCAST_DATA SNR=7.5",
            "DATAC3DATAC3 BCAST_DATA LEN=124 SNR=7.5",
            "",
        ] {
            assert_eq!(parse_broadcast(line), None, "{line:?}");
        }
    }
}
