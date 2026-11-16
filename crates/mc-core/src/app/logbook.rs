//! QSO records as the UI sees them (LOG-01): one draft per session, with the
//! frequency's provenance. Nothing here is guessed: a frequency Mercury did
//! not report fresh is `None`, never 0.

use super::discovery::Reading;
use super::telemetry::Tuning;
use crate::band::band_for;
use crate::profile::FieldError;
use mc_storage::{QsoEdit as RowEdit, QsoRow};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// `freq_source` of a frequency the operator typed.
pub const OPERATOR_SOURCE: &str = "operator";
/// Setting: end-of-session records become final by themselves (default off).
pub const FINALISE_SETTING: &str = "log.finalise_on_disconnect";

const TEXT_MAX: usize = 120;
const NOTES_MAX: usize = 2000;
const FREQ_MAX_HZ: u64 = 1_000_000_000;
const MODE_MAX_LEN: usize = 24;
const MODES_MAX: usize = 32;

/// A QSO record: the row's fields, with the frequency as a reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Qso {
    pub id: String,
    pub session_id: Option<String>,
    pub peer: String,
    pub start_utc: String,
    pub end_utc: Option<String>,
    pub grid: Option<String>,
    pub band: Option<String>,
    /// Where the radio was: `mercury telemetry` (with its age) or `operator`.
    #[ts(as = "Option<Reading<f64>>")]
    pub freq: Option<Reading<u64>>,
    pub bandwidth: Option<String>,
    /// Modes observed during the session, comma separated, in order seen.
    pub tx_modes: Option<String>,
    pub rx_modes: Option<String>,
    pub rst_sent: Option<String>,
    pub rst_rcvd: Option<String>,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub my_power: Option<String>,
    pub notes: Option<String>,
    /// `draft` or `final`.
    pub state: String,
}

impl From<&QsoRow> for Qso {
    fn from(r: &QsoRow) -> Qso {
        let freq = match (r.freq_hz.and_then(|h| u64::try_from(h).ok()), &r.freq_source) {
            (Some(value), Some(source)) => Some(Reading {
                value,
                source: source.clone(),
                at_utc: r.start_utc.clone(),
                age_ms: r.freq_age_ms.and_then(|a| u64::try_from(a).ok()),
            }),
            _ => None,
        };
        Qso {
            id: r.id.clone(),
            session_id: r.session_id.clone(),
            peer: r.peer.clone(),
            start_utc: r.start_utc.clone(),
            end_utc: r.end_utc.clone(),
            grid: r.grid.clone(),
            band: r.band.clone(),
            freq,
            bandwidth: r.bandwidth.clone(),
            tx_modes: r.tx_modes.clone(),
            rx_modes: r.rx_modes.clone(),
            rst_sent: r.rst_sent.clone(),
            rst_rcvd: r.rst_rcvd.clone(),
            name: r.name.clone(),
            qth: r.qth.clone(),
            my_power: r.my_power.clone(),
            notes: r.notes.clone(),
            state: r.state.clone(),
        }
    }
}

/// What the operator may change on a draft. Only `Some` fields are written;
/// empty text is ignored. The session facts (peer, times, bandwidth, modes)
/// are not editable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QsoEdit {
    pub grid: Option<String>,
    /// Only meaningful with no frequency; a typed frequency sets its own band.
    pub band: Option<String>,
    /// A typed dial frequency in Hz; stored with source `operator`.
    #[ts(type = "number | null")]
    pub freq_hz: Option<u64>,
    pub rst_sent: Option<String>,
    pub rst_rcvd: Option<String>,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub my_power: Option<String>,
    pub notes: Option<String>,
}

fn text(
    errors: &mut Vec<FieldError>,
    field: &str,
    v: &Option<String>,
    max: usize,
    allowed: impl Fn(char) -> bool,
) -> Option<String> {
    let t = v.as_deref()?.trim();
    if t.is_empty() {
        return None;
    }
    if t.chars().count() > max || !t.chars().all(allowed) {
        errors.push(FieldError { field: field.into(), message: format!("not valid text, at most {max} characters") });
        return None;
    }
    Some(t.to_string())
}

fn printable(c: char) -> bool {
    !c.is_control()
}

impl QsoEdit {
    /// Check the edit and turn it into the storage's. A typed frequency also
    /// sets the band it falls in (none if outside every band), the source
    /// `operator` and no age.
    pub fn validate(&self) -> Result<RowEdit, Vec<FieldError>> {
        let mut errors = Vec::new();
        let mut e = RowEdit {
            grid: text(&mut errors, "grid", &self.grid, 8, |c| c.is_ascii_alphanumeric()),
            band: text(&mut errors, "band", &self.band, 8, |c| c.is_ascii_alphanumeric()),
            rst_sent: text(&mut errors, "rstSent", &self.rst_sent, 8, |c| c.is_ascii_alphanumeric()),
            rst_rcvd: text(&mut errors, "rstRcvd", &self.rst_rcvd, 8, |c| c.is_ascii_alphanumeric()),
            name: text(&mut errors, "name", &self.name, TEXT_MAX, printable),
            qth: text(&mut errors, "qth", &self.qth, TEXT_MAX, printable),
            my_power: text(&mut errors, "myPower", &self.my_power, 16, printable),
            notes: text(&mut errors, "notes", &self.notes, NOTES_MAX, |c| !c.is_control() || c == '\n'),
            ..RowEdit::default()
        };
        if let Some(hz) = self.freq_hz {
            if hz == 0 || hz > FREQ_MAX_HZ {
                errors.push(FieldError { field: "freqHz".into(), message: "not a frequency in Hz".into() });
            } else {
                e.freq_hz = Some(hz as i64);
                e.freq_source = Some(OPERATOR_SOURCE.into());
                e.freq_age_ms = None;
                e.band = band_for(hz).map(str::to_string);
            }
        }
        if errors.is_empty() {
            Ok(e)
        } else {
            Err(errors)
        }
    }
}

/// A new draft for a session that just started.
pub struct Start<'a> {
    pub session_id: &'a str,
    pub peer: &'a str,
    pub start_utc: &'a str,
    pub bandwidth: &'a str,
    /// The peer's locator from the heard list, if it has one.
    pub grid: Option<String>,
    /// Mercury's frequency, only if fresh (`Telemetry::tuning`).
    pub tuning: &'a Tuning,
}

pub fn draft_row(s: &Start<'_>) -> QsoRow {
    let freq = s.tuning.freq.as_ref();
    QsoRow {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: Some(s.session_id.into()),
        peer: s.peer.into(),
        start_utc: s.start_utc.into(),
        end_utc: None,
        grid: s.grid.clone(),
        band: freq.and_then(|f| band_for(f.value)).map(str::to_string),
        freq_hz: freq.and_then(|f| i64::try_from(f.value).ok()),
        freq_source: freq.map(|f| f.source.clone()),
        freq_age_ms: freq.and_then(|f| f.age_ms).and_then(|a| i64::try_from(a).ok()),
        bandwidth: Some(s.bandwidth.to_string()).filter(|b| !b.is_empty()),
        tx_modes: None,
        rx_modes: None,
        rst_sent: None,
        rst_rcvd: None,
        name: None,
        qth: None,
        my_power: None,
        notes: None,
        state: "draft".into(),
    }
}

/// `list` with `mode` added once. `None` when nothing changes: the mode is
/// already there, or it does not look like a Mercury mode name.
pub fn with_mode(list: Option<&str>, mode: &str) -> Option<String> {
    let ok = !mode.is_empty()
        && mode.len() <= MODE_MAX_LEN
        && mode.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok {
        return None;
    }
    let have: Vec<&str> = list.map(|l| l.split(',').collect()).unwrap_or_default();
    if have.contains(&mode) || have.len() >= MODES_MAX {
        return None;
    }
    Some(if have.is_empty() { mode.to_string() } else { format!("{},{mode}", have.join(",")) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_are_appended_once_and_bounded() {
        assert_eq!(with_mode(None, "DATAC3"), Some("DATAC3".into()));
        assert_eq!(with_mode(Some("DATAC3"), "DATAC1"), Some("DATAC3,DATAC1".into()));
        assert_eq!(with_mode(Some("DATAC3,DATAC1"), "DATAC3"), None);
        assert_eq!(with_mode(None, ""), None);
        assert_eq!(with_mode(None, "a,b"), None);
        assert_eq!(with_mode(None, &"X".repeat(25)), None);
    }

    #[test]
    fn operator_frequency_sets_source_band_and_clears_age() {
        let e = QsoEdit { freq_hz: Some(7_074_000), ..Default::default() }.validate().unwrap();
        assert_eq!(
            (e.freq_hz, e.freq_source.as_deref(), e.freq_age_ms, e.band.as_deref()),
            (Some(7_074_000), Some("operator"), None, Some("40m"))
        );
        assert!(QsoEdit { freq_hz: Some(0), ..Default::default() }.validate().is_err());
    }
}
