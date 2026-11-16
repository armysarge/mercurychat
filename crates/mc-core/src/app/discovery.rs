//! Discovery as the UI sees it (DSC-01, DSC-04): the heard list built from
//! CQ, MCB/1 beacon and session observations, favourites, and the activity
//! log. Everything from the air is untrusted: callsigns pass
//! `Callsign::parse` before anything is stored, and every time is this
//! station's own UTC at receipt (no sender clock).

use super::events::{AppEvent, ModemState};
use crate::station::Observation;
use mc_storage::{now_utc, HeardRow, HeardSummaryRow, StorageError, Store};
use mch_protocol::beacon::{self, Received};
use mercury_adapter::kiss::{KissFrame, CMD_DATA_STD};
use mercury_adapter::Callsign;
use serde::Serialize;
use std::collections::VecDeque;
use ts_rs::TS;

/// The activity ring keeps this many entries, newest last.
pub const ACTIVITY_RING: usize = 500;
/// A MONITOR line updates the SNR of that call's CQ row from this many
/// seconds back; otherwise it is an observation of its own.
pub const MONITOR_MERGE_S: i64 = 10;
/// Detailed heard observations are kept this long (02 §6).
pub const HEARD_RETENTION_DAYS: i64 = 30;
/// The provenance of an SNR taken from a MONITOR line (DSC-04).
pub const MONITOR_SNR: &str = "modem monitor";

/// A measured value with its provenance (DSC-04): where it came from, when
/// this station took it, and the source's own age where it reports one.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Reading<T> {
    pub value: T,
    pub source: String,
    pub at_utc: String,
    #[ts(type = "number | null")]
    pub age_ms: Option<u64>,
}

// Readings are only ever built from finite values (NaN never gets this far),
// so equality is total.
impl Eq for Reading<f32> {}

/// How a station was last heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HeardSource {
    Cq,
    Beacon,
    Session,
}

impl HeardSource {
    fn from_kind(kind: &str) -> Option<HeardSource> {
        match kind {
            "cq" => Some(HeardSource::Cq),
            "beacon" => Some(HeardSource::Beacon),
            "session" => Some(HeardSource::Session),
            _ => None,
        }
    }
}

/// One row of the heard list: a station's latest observation (DSC-01).
/// Heard is not the same as reachable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HeardStation {
    pub callsign: String,
    /// When this station last heard it, by its own clock. `None` for a
    /// favourite with nothing heard in the retention window: no time is
    /// made up.
    pub last_heard_utc: Option<String>,
    /// `None` exactly when `last_heard_utc` is.
    pub source: Option<HeardSource>,
    pub grid: Option<String>,
    pub caps: Option<u16>,
    /// Observations kept for this callsign.
    pub count: u32,
    pub favourite: bool,
    pub snr: Option<Reading<f32>>,
    pub bandwidth: Option<String>,
}

/// One line of the activity log. `kind` is a short lowercase word: state,
/// ptt, busy, pending, buffer, heard, malformed, unsupported, broadcast or
/// purge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ActivityEntry {
    pub at_utc: String,
    pub kind: String,
    pub text: String,
}

/// Broadcast traffic that was not a station: malformed MCB/1 or invalid
/// callsigns (`malformed`), and other applications' frames (`unsupported`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiscoveryStats {
    pub malformed: u32,
    pub unsupported: u32,
}

/// Untrusted text from the air, made safe to show: bounded and escaped.
fn shown(s: &str) -> String {
    let cut: String = s.chars().take(32).collect();
    let more = if s.chars().count() > 32 { "…" } else { "" };
    format!("\"{}{more}\"", cut.escape_debug())
}

/// A bandwidth token from the air, kept only if it looks like one.
fn bandwidth(bw: &str) -> Option<String> {
    (!bw.is_empty() && bw.len() <= 8 && bw.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| bw.to_string())
}

fn row(callsign: &str, kind: &str) -> HeardRow {
    HeardRow {
        id: 0,
        callsign: callsign.to_string(),
        observed_utc: now_utc(),
        kind: kind.to_string(),
        bandwidth: None,
        grid: None,
        caps: None,
        snr_db: None,
        snr_source: None,
        freq_hz: None,
        freq_source: None,
        freq_age_ms: None,
    }
}

fn station(r: HeardSummaryRow) -> HeardStation {
    let snr = r.snr_db.filter(|v| v.is_finite()).map(|value| Reading {
        value,
        source: r.snr_source.clone().unwrap_or_else(|| "unknown".into()),
        at_utc: r.last_utc.clone(),
        age_ms: None,
    });
    HeardStation {
        source: HeardSource::from_kind(&r.kind),
        last_heard_utc: Some(r.last_utc),
        callsign: r.callsign,
        grid: r.grid,
        caps: r.caps.and_then(|c| u16::try_from(c).ok()),
        count: u32::try_from(r.count).unwrap_or(u32::MAX),
        favourite: r.favourite,
        snr,
        bandwidth: r.bandwidth,
    }
}

/// A callsign with nothing heard: a favourite, or one just unmarked.
fn unheard(callsign: &str, favourite: bool) -> HeardStation {
    HeardStation {
        callsign: callsign.to_string(),
        last_heard_utc: None,
        source: None,
        grid: None,
        caps: None,
        count: 0,
        favourite,
        snr: None,
        bandwidth: None,
    }
}

/// The heard list: every station heard in the retention window, most
/// recently heard first, then the favourites with nothing heard.
pub fn heard_list(store: &Store) -> Result<Vec<HeardStation>, StorageError> {
    let mut list: Vec<HeardStation> = store.heard_summary()?.into_iter().map(station).collect();
    for call in store.favourites()? {
        if !list.iter().any(|s| s.callsign == call) {
            list.push(unheard(&call, true));
        }
    }
    Ok(list)
}

/// One callsign as the heard list shows it. A callsign neither heard nor a
/// favourite comes back with `count` 0 and `favourite` false: not listed.
pub fn heard_station(store: &Store, callsign: &str) -> Result<HeardStation, StorageError> {
    if let Some(r) = store.heard_summary()?.into_iter().find(|r| r.callsign == callsign) {
        return Ok(station(r));
    }
    let favourite = store.favourites()?.iter().any(|c| c == callsign);
    Ok(unheard(callsign, favourite))
}

/// The actor's discovery state: the activity ring, the counters and the
/// last BUFFER value. Each method returns the events to publish.
pub struct Discovery {
    ring: VecDeque<ActivityEntry>,
    stats: DiscoveryStats,
    buffer: Option<u64>,
}

impl Default for Discovery {
    fn default() -> Self {
        Discovery { ring: VecDeque::with_capacity(ACTIVITY_RING), stats: DiscoveryStats::default(), buffer: None }
    }
}

impl Discovery {
    pub fn stats(&self) -> DiscoveryStats {
        self.stats
    }

    pub fn entries(&self) -> Vec<ActivityEntry> {
        self.ring.iter().cloned().collect()
    }

    /// A new modem task: BUFFER is compared afresh.
    pub fn modem_changed(&mut self) {
        self.buffer = None;
    }

    /// Add an activity line; the returned event publishes it.
    pub fn note(&mut self, kind: &str, text: impl Into<String>) -> AppEvent {
        let entry = ActivityEntry { at_utc: now_utc(), kind: kind.to_string(), text: text.into() };
        if self.ring.len() >= ACTIVITY_RING {
            self.ring.pop_front();
        }
        self.ring.push_back(entry.clone());
        AppEvent::Activity { entry }
    }

    pub fn state(&mut self, state: ModemState, detail: Option<&str>) -> AppEvent {
        let word = format!("{state:?}").to_lowercase();
        let text = match detail {
            Some(d) => format!("modem {word}: {d}"),
            None => format!("modem {word}"),
        };
        self.note("state", text)
    }

    fn malformed(&mut self, text: String) -> Vec<AppEvent> {
        self.stats.malformed = self.stats.malformed.saturating_add(1);
        vec![self.note("malformed", text)]
    }

    fn unsupported(&mut self, text: String) -> Vec<AppEvent> {
        self.stats.unsupported = self.stats.unsupported.saturating_add(1);
        vec![self.note("unsupported", text)]
    }

    /// A callsign from the air, or a malformed count and line.
    fn callsign(&mut self, raw: &str, what: &str) -> Result<String, Vec<AppEvent>> {
        Callsign::parse(raw)
            .map(|c| c.as_str().to_string())
            .map_err(|_| self.malformed(format!("{what} from an invalid callsign {}", shown(raw))))
    }

    /// The activity line and heard-list update after `callsign` was stored.
    fn heard(&mut self, store: &Store, callsign: &str, how: &str) -> Vec<AppEvent> {
        let mut out = vec![self.note("heard", format!("heard {callsign} ({how})"))];
        match heard_station(store, callsign) {
            Ok(station) => out.push(AppEvent::Heard { station }),
            Err(e) => tracing::warn!("could not read the heard list: {e}"),
        }
        out
    }

    /// Store one observation (a single short insert) and publish it.
    fn record(&mut self, store: Option<&mut Store>, row: HeardRow, how: &str) -> Vec<AppEvent> {
        let Some(store) = store else { return Vec::new() };
        if let Err(e) = store.record_heard(&row) {
            tracing::warn!("could not record a heard station: {e}");
            return Vec::new();
        }
        self.heard(store, &row.callsign, how)
    }

    /// One observation from the modem's control stream. Heard rows need the
    /// station's store; without one they are dropped.
    pub fn observation(&mut self, store: Option<&mut Store>, o: Observation) -> Vec<AppEvent> {
        match o {
            Observation::Cq { source, bw } => {
                let call = match self.callsign(&source, "CQ") {
                    Ok(c) => c,
                    Err(evs) => return evs,
                };
                let row = HeardRow { bandwidth: bandwidth(&bw), ..row(&call, "cq") };
                self.record(store, row, "CQ")
            }
            Observation::MonitorCq(m) => {
                let call = match self.callsign(&m.from, "monitored CQ") {
                    Ok(c) => c,
                    Err(evs) => return evs,
                };
                let Some(store) = store else { return Vec::new() };
                let how = format!("CQ, SNR {:.1} dB", m.snr_db);
                let merged = match store.latest_heard(&call, "cq", MONITOR_MERGE_S) {
                    Ok(Some(r)) => store.set_heard_snr(r.id, m.snr_db, MONITOR_SNR).map(|_| true),
                    Ok(None) => Ok(false),
                    Err(e) => Err(e),
                };
                match merged {
                    Ok(true) => self.heard(store, &call, &how),
                    Ok(false) => {
                        let row = HeardRow {
                            bandwidth: bandwidth(&m.bw),
                            snr_db: Some(m.snr_db),
                            snr_source: Some(MONITOR_SNR.into()),
                            ..row(&call, "cq")
                        };
                        self.record(Some(store), row, &how)
                    }
                    Err(e) => {
                        tracing::warn!("could not record a monitored CQ: {e}");
                        Vec::new()
                    }
                }
            }
            Observation::Ptt(on) => vec![self.note("ptt", if on { "PTT on" } else { "PTT off" })],
            Observation::Busy(on) => vec![self.note("busy", if on { "channel busy" } else { "channel clear" })],
            Observation::Pending => vec![self.note("pending", "modem pending: a transmission is queued")],
            Observation::CancelPending => vec![self.note("pending", "modem pending cleared")],
            Observation::Buffer(n) if self.buffer != Some(n) => {
                self.buffer = Some(n);
                vec![self.note("buffer", format!("modem buffer: {n} bytes"))]
            }
            Observation::Buffer(_) => Vec::new(),
            // A CQ line the adapter could not read is malformed traffic; other
            // unknown lines (MONITOR of other modes, new keywords) are not
            // discovery and stay out of the log.
            Observation::Unknown(line) if line.starts_with("CQFRAME") => {
                self.malformed(format!("unreadable CQ {}", shown(&line)))
            }
            Observation::Unknown(_) | Observation::Snr(_) => Vec::new(),
        }
    }

    /// One KISS frame from the broadcast port (03 §6).
    pub fn frame(&mut self, store: Option<&mut Store>, f: KissFrame) -> Vec<AppEvent> {
        if f.cmd != CMD_DATA_STD {
            return self.unsupported(format!("broadcast frame with KISS command 0x{:02x}", f.cmd));
        }
        match beacon::classify(&f.payload) {
            Received::Beacon(b) => {
                let call = match self.callsign(&b.callsign, "beacon") {
                    Ok(c) => c,
                    Err(evs) => return evs,
                };
                let row = HeardRow { grid: b.grid, caps: Some(i64::from(b.caps)), ..row(&call, "beacon") };
                self.record(store, row, "beacon")
            }
            Received::Malformed(e) => self.malformed(format!("malformed MCB/1 beacon: {e}")),
            Received::Foreign => {
                self.unsupported(format!("another application's broadcast ({} bytes)", f.payload.len()))
            }
        }
    }

    /// A negotiated session with `peer`.
    pub fn session(&mut self, store: Option<&mut Store>, peer: &str) -> Vec<AppEvent> {
        match self.callsign(peer, "session") {
            Ok(call) => self.record(store, row(&call, "session"), "session"),
            Err(evs) => evs,
        }
    }

    /// Drop heard observations past retention (on opening a station store).
    pub fn purge(&mut self, store: &mut Store) -> Vec<AppEvent> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(HEARD_RETENTION_DAYS))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        match store.purge_heard_before(&cutoff) {
            Ok(0) => Vec::new(),
            Ok(n) => {
                vec![self
                    .note("purge", format!("removed {n} heard observation(s) older than {HEARD_RETENTION_DAYS} days"))]
            }
            Err(e) => {
                tracing::warn!("could not purge old heard observations: {e}");
                Vec::new()
            }
        }
    }
}
