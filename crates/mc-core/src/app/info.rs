//! Information requests (M3c B6, B10), the pure part: what the operator
//! shares, what an answer carries, and how often one is given.

use super::discovery::Reading;
use crate::profile::StationProfile;
use mc_storage::Store;
use mch_protocol::payload::{InfoWhat, SnrReport, StationShare};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::Instant;
use ts_rs::TS;

/// One answer of each kind per this long, per session; extra requests get
/// an empty answer marked `rate`.
pub const RATE: Duration = Duration::from_secs(10);
/// An SNR older than this is no reading (B10).
pub const MAX_SNR_AGE: Duration = Duration::from_secs(60);
/// The source a peer's report is recorded under on the QSO card.
pub const PEER_REPORT: &str = "peer report";

/// A station field the operator may share (Settings → Station → Share).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ShareField {
    Name,
    Qth,
    Locator,
    Rig,
    Power,
    Antenna,
    About,
}

/// The fields answered to a station-info request. Email is never one: it
/// goes out only with "Send my email" (B6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShareConfig {
    pub fields: Vec<ShareField>,
}

impl Default for ShareConfig {
    /// Name, QTH and locator on, the rest off (B6).
    fn default() -> Self {
        ShareConfig { fields: vec![ShareField::Name, ShareField::Qth, ShareField::Locator] }
    }
}

fn share_key(profile_id: &str) -> String {
    format!("profile.{profile_id}.share")
}

impl ShareConfig {
    /// The profile's choice, or the defaults when none is stored (or it does not read).
    pub fn load(store: &Store, profile_id: &str) -> ShareConfig {
        store
            .setting(&share_key(profile_id))
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, store: &mut Store, profile_id: &str) -> Result<(), mc_storage::StorageError> {
        let json = serde_json::to_string(self).expect("a share config serialises");
        store.set_setting(&share_key(profile_id), &json)
    }
}

/// The station answer: the shared fields that are set and within their B10
/// limits. A value over its limit (stored before M3c) is left out, never truncated.
pub fn station_share(p: &StationProfile, cfg: &ShareConfig) -> StationShare {
    let limit = |field: &str| StationShare::LIMITS.iter().find(|(f, _)| *f == field).map_or(0, |(_, n)| *n);
    let take = |on: ShareField, v: &Option<String>, field: &str| {
        v.as_deref()
            .filter(|_| cfg.fields.contains(&on))
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= limit(field))
            .map(String::from)
    };
    let locator = p
        .grid
        .as_deref()
        .filter(|_| cfg.fields.contains(&ShareField::Locator))
        .filter(|g| mch_protocol::beacon::valid_grid(g))
        .map(String::from);
    StationShare {
        name: take(ShareField::Name, &p.name, "name"),
        qth: take(ShareField::Qth, &p.qth, "qth"),
        locator,
        rig: take(ShareField::Rig, &p.rig, "rig"),
        power: take(ShareField::Power, &p.power, "power"),
        antenna: take(ShareField::Antenna, &p.antenna, "antenna"),
        about: take(ShareField::About, &p.about, "about"),
        email: None,
    }
}

/// The SNR answer from the best reading (telemetry, else the control port's
/// SN line), or `Err("no-reading")` when there is none younger than 60 s.
pub fn snr_report(snr: Option<&Reading<f32>>, now_utc: &str) -> Result<SnrReport, &'static str> {
    let r = snr.ok_or("no-reading")?;
    let age_ms = r.age_ms.or_else(|| {
        let at = chrono::DateTime::parse_from_rfc3339(&r.at_utc).ok()?;
        let now = chrono::DateTime::parse_from_rfc3339(now_utc).ok()?;
        u64::try_from((now - at).num_milliseconds()).ok()
    });
    let age = Duration::from_millis(age_ms.ok_or("no-reading")?);
    if age > MAX_SNR_AGE || !r.value.is_finite() {
        return Err("no-reading");
    }
    // The answer's source is a short token (at most 16 bytes on the wire).
    let source = if r.source == super::telemetry::TELEMETRY_SOURCE { "telemetry" } else { "sn" };
    Ok(SnrReport { db: (r.value * 10.0).round() / 10.0, age_s: age.as_secs() as u32, source: source.into() })
}

/// One answer per kind every `RATE`, per session.
#[derive(Debug, Default)]
pub struct InfoLimiter {
    last: [Option<Instant>; 2],
}

impl InfoLimiter {
    pub fn allow(&mut self, what: InfoWhat, now: Instant) -> bool {
        let slot = &mut self.last[what as usize];
        if slot.is_some_and(|at| now.saturating_duration_since(at) < RATE) {
            return false;
        }
        *slot = Some(now);
        true
    }
}

/// A peer's SNR report as the UI shows it: "ZS6ABC reports your signal at −3 dB".
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerSnr {
    pub db: f32,
    pub age_s: u32,
    pub source: String,
}

// Built only from finite values (InfoResponse::validate refuses others).
impl Eq for PeerSnr {}

/// The station fields a peer shared.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PeerStation {
    pub name: Option<String>,
    pub qth: Option<String>,
    pub locator: Option<String>,
    pub rig: Option<String>,
    pub power: Option<String>,
    pub antenna: Option<String>,
    pub about: Option<String>,
    pub email: Option<String>,
}

impl From<SnrReport> for PeerSnr {
    fn from(r: SnrReport) -> Self {
        PeerSnr { db: r.db, age_s: r.age_s, source: r.source }
    }
}

impl From<StationShare> for PeerStation {
    fn from(s: StationShare) -> Self {
        PeerStation {
            name: s.name,
            qth: s.qth,
            locator: s.locator,
            rig: s.rig,
            power: s.power,
            antenna: s.antenna,
            about: s.about,
            email: s.email,
        }
    }
}

/// The QSO card's RST received from a peer's SNR report.
pub fn rst_from_snr(db: f32) -> String {
    format!("{db:+.0} dB")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(value: f32, age_ms: Option<u64>, source: &str) -> Reading<f32> {
        Reading { value, source: source.into(), at_utc: "2026-11-21T10:00:00.000Z".into(), age_ms }
    }

    #[test]
    fn snr_ages_and_sources() {
        let now = "2026-11-21T10:00:30.000Z";
        let t = reading(-3.4, Some(12_000), super::super::telemetry::TELEMETRY_SOURCE);
        let r = snr_report(Some(&t), now).unwrap();
        assert_eq!((r.db, r.age_s, r.source.as_str()), (-3.4, 12, "telemetry"));
        let sn = reading(5.0, None, super::super::telemetry::CONTROL_SOURCE);
        assert_eq!(snr_report(Some(&sn), now).unwrap().age_s, 30, "aged from its time when no age is known");
        assert_eq!(snr_report(Some(&sn), "2026-11-21T10:01:01.000Z"), Err("no-reading"));
        assert_eq!(snr_report(None, now), Err("no-reading"));
    }

    #[test]
    fn limiter_is_per_kind() {
        let mut l = InfoLimiter::default();
        let t0 = Instant::now();
        assert!(l.allow(InfoWhat::Snr, t0));
        assert!(!l.allow(InfoWhat::Snr, t0 + Duration::from_secs(9)));
        assert!(l.allow(InfoWhat::Station, t0 + Duration::from_secs(1)));
        assert!(l.allow(InfoWhat::Snr, t0 + Duration::from_secs(10)));
    }

    #[test]
    fn rst_is_signed_whole_db() {
        assert_eq!(
            (rst_from_snr(-3.4), rst_from_snr(7.6), rst_from_snr(0.0)),
            ("-3 dB".into(), "+8 dB".into(), "+0 dB".into())
        );
    }
}
