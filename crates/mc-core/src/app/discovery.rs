//! Discovery as the UI sees it (DSC-01, DSC-04): the heard list built from
//! CQ, MCB/1 beacon and session observations, favourites, and the activity
//! log. Everything from the air is untrusted: callsigns pass
//! `Callsign::parse` before anything is stored, and every time is this
//! station's own UTC at receipt (no sender clock).

use super::events::{AppEvent, ModemState};
use super::telemetry::Tuning;
use crate::band::band_for;
use crate::station::Observation;
use mc_storage::{now_utc, HeardRow, HeardSummaryRow, StorageError, Store};
use mch_protocol::beacon::{self, Received};
use mercury_adapter::kiss::{KissFrame, CMD_DATA_STD};
use mercury_adapter::Callsign;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::time::Instant;
use ts_rs::TS;

/// The activity ring keeps this many entries, newest last.
pub const ACTIVITY_RING: usize = 500;
/// One CQ comes as two control lines, `CQFRAME` and `MONITOR … CQ`, in the
/// same read and in either order (lab Q3). The second of them merges into
/// the row the first one made if it comes within this window; otherwise it
/// is a CQ of its own.
pub const CQ_PAIR: std::time::Duration = std::time::Duration::from_secs(5);
/// At most this many CQs wait for their other line (in memory only).
const CQ_PAIRS_KEPT: usize = 64;
/// Detailed heard observations are kept this long (02 §6).
pub const HEARD_RETENTION_DAYS: i64 = 30;
/// The provenance of an SNR taken from a MONITOR line (DSC-04).
pub const MONITOR_SNR: &str = "modem monitor";
/// At most this many stations' carrier offsets are kept (in memory only).
pub const OFFSETS_KEPT: usize = 1024;

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
impl Eq for Reading<u64> {}
impl Eq for Reading<String> {}

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
    /// The radio's dial frequency (Hz) when this station was last heard, if
    /// Mercury's telemetry had one no older than 10 s then.
    #[ts(as = "Option<Reading<f64>>")]
    pub freq: Option<Reading<u64>>,
    /// The amateur band of `freq` (e.g. `20m`).
    pub band: Option<String>,
    /// How far off nominal the last frame heard from it arrived (Hz), from
    /// Mercury's telemetry when under 2 s old as that frame arrived. Mercury
    /// has no slots: this is an offset, nothing more.
    pub offset_hz: Option<Reading<f32>>,
    /// Name, QTH and status from its last MCB/1 kind 2 beacon (source
    /// `beacon`), kept for this modem only (M3c B7).
    pub name: Option<Reading<String>>,
    pub qth: Option<Reading<String>>,
    pub status: Option<Reading<String>>,
}

/// One line of the activity log. `kind` is a short lowercase word: state,
/// ptt, busy, pending, buffer, heard, malformed, unsupported, broadcast,
/// message (a broadcast message the operator sent), telemetry or purge.
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

fn station(r: HeardSummaryRow, offsets: &Offsets) -> HeardStation {
    let snr = r.snr_db.filter(|v| v.is_finite()).map(|value| Reading {
        value,
        source: r.snr_source.clone().unwrap_or_else(|| "unknown".into()),
        at_utc: r.last_utc.clone(),
        age_ms: None,
    });
    let freq = r.freq_hz.and_then(|hz| u64::try_from(hz).ok()).map(|value| Reading {
        value,
        source: r.freq_source.clone().unwrap_or_else(|| "unknown".into()),
        at_utc: r.last_utc.clone(),
        age_ms: r.freq_age_ms.and_then(|a| u64::try_from(a).ok()),
    });
    let band = freq.as_ref().and_then(|f| band_for(f.value)).map(String::from);
    let offset_hz = offsets.get(&r.callsign, &r.last_utc);
    let fields = offsets.fields.get(&r.callsign).cloned().unwrap_or_default();
    HeardStation {
        name: fields.name,
        qth: fields.qth,
        status: fields.status,
        freq,
        band,
        offset_hz,
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
        freq: None,
        band: None,
        offset_hz: None,
        name: None,
        qth: None,
        status: None,
    }
}

/// The carrier offset of the frame each station was last heard by, kept in
/// memory for this modem only: (that observation's time, the offset).
#[derive(Default)]
pub struct Offsets {
    offsets: HashMap<String, (String, Reading<f32>)>,
    /// Each station's last kind 2 fields (a kind 1 beacon clears them).
    fields: HashMap<String, BeaconFields>,
}

#[derive(Debug, Clone, Default)]
struct BeaconFields {
    name: Option<Reading<String>>,
    qth: Option<Reading<String>>,
    status: Option<Reading<String>>,
}

impl Offsets {
    /// The offset of `callsign`'s observation at `observed_utc`, if it had one.
    fn get(&self, callsign: &str, observed_utc: &str) -> Option<Reading<f32>> {
        self.offsets.get(callsign).filter(|(at, _)| at == observed_utc).map(|(_, r)| r.clone())
    }

    /// Record (or, with `None`, clear) the offset of `callsign`'s newest
    /// observation. The oldest entry makes way when the map is full.
    fn set(&mut self, callsign: &str, observed_utc: &str, offset: Option<Reading<f32>>) {
        let Some(mut r) = offset else {
            self.offsets.remove(callsign);
            return;
        };
        if self.offsets.len() >= OFFSETS_KEPT && !self.offsets.contains_key(callsign) {
            let oldest = self.offsets.iter().min_by(|a, b| a.1 .0.cmp(&b.1 .0)).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.offsets.remove(&k);
            }
        }
        r.at_utc = observed_utc.to_string();
        self.offsets.insert(callsign.to_string(), (observed_utc.to_string(), r));
    }

    /// What `callsign`'s beacon heard at `at_utc` carried; the oldest
    /// station makes way when the map is full.
    fn set_fields(&mut self, callsign: &str, at_utc: &str, tlvs: &[beacon::Tlv]) {
        let reading =
            |v: &str| Reading { value: v.to_string(), source: "beacon".into(), at_utc: at_utc.into(), age_ms: None };
        let mut f = BeaconFields::default();
        for t in tlvs {
            match t {
                beacon::Tlv::Name(v) => f.name = Some(reading(v)),
                beacon::Tlv::Qth(v) => f.qth = Some(reading(v)),
                beacon::Tlv::Status(v) => f.status = Some(reading(v)),
                beacon::Tlv::Unknown(..) => {}
            }
        }
        if f.name.is_none() && f.qth.is_none() && f.status.is_none() {
            self.fields.remove(callsign);
            return;
        }
        if self.fields.len() >= OFFSETS_KEPT && !self.fields.contains_key(callsign) {
            let oldest = self
                .fields
                .iter()
                .min_by_key(|(_, f)| {
                    [&f.name, &f.qth, &f.status].iter().find_map(|r| r.as_ref().map(|r| r.at_utc.clone()))
                })
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.fields.remove(&k);
            }
        }
        self.fields.insert(callsign.to_string(), f);
    }
}

/// The heard list: every station heard in the retention window, most
/// recently heard first, then the favourites with nothing heard.
pub fn heard_list(store: &Store, offsets: &Offsets) -> Result<Vec<HeardStation>, StorageError> {
    let mut list: Vec<HeardStation> = store.heard_summary()?.into_iter().map(|r| station(r, offsets)).collect();
    for call in store.favourites()? {
        if !list.iter().any(|s| s.callsign == call) {
            list.push(unheard(&call, true));
        }
    }
    Ok(list)
}

/// One callsign as the heard list shows it. A callsign neither heard nor a
/// favourite comes back with `count` 0 and `favourite` false: not listed.
pub fn heard_station(store: &Store, callsign: &str, offsets: &Offsets) -> Result<HeardStation, StorageError> {
    if let Some(r) = store.heard_summary()?.into_iter().find(|r| r.callsign == callsign) {
        return Ok(station(r, offsets));
    }
    let favourite = store.favourites()?.iter().any(|c| c == callsign);
    Ok(unheard(callsign, favourite))
}

/// The actor's discovery state: the activity ring, the counters, the last
/// BUFFER value and the heard stations' offsets. Each method returns the
/// events to publish.
pub struct Discovery {
    ring: VecDeque<ActivityEntry>,
    stats: DiscoveryStats,
    buffer: Option<u64>,
    offsets: Offsets,
    /// CQ rows made by one line of a CQ, waiting for the other.
    cq_halves: HashMap<String, HalfCq>,
}

/// Which of a CQ's two control lines made its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CqLine {
    Frame,
    Monitor,
}

/// A CQ row waiting for the CQ's other line (lab Q3: either order).
struct HalfCq {
    id: i64,
    observed_utc: String,
    first: CqLine,
    at: Instant,
}

impl Default for Discovery {
    fn default() -> Self {
        Discovery {
            ring: VecDeque::with_capacity(ACTIVITY_RING),
            stats: DiscoveryStats::default(),
            buffer: None,
            offsets: Offsets::default(),
            cq_halves: HashMap::new(),
        }
    }
}

impl Discovery {
    pub fn stats(&self) -> DiscoveryStats {
        self.stats
    }

    pub fn entries(&self) -> Vec<ActivityEntry> {
        self.ring.iter().cloned().collect()
    }

    /// The heard stations' offsets, for listing them.
    pub fn offsets(&self) -> &Offsets {
        &self.offsets
    }

    /// A new modem task: BUFFER is compared afresh, and offsets heard
    /// through the old one are forgotten.
    pub fn modem_changed(&mut self) {
        self.buffer = None;
        self.offsets = Offsets::default();
        self.cq_halves.clear();
    }

    /// The row `call`'s CQ made from its `first` line within [`CQ_PAIR`], if
    /// any: taken, because each CQ pairs once.
    fn take_half(&mut self, call: &str, first: CqLine) -> Option<HalfCq> {
        let now = Instant::now();
        self.cq_halves.retain(|_, h| now.duration_since(h.at) < CQ_PAIR);
        if self.cq_halves.get(call).is_some_and(|h| h.first == first) {
            return self.cq_halves.remove(call);
        }
        None
    }

    /// Remember the row one line of `call`'s CQ made, for the other line.
    fn keep_half(&mut self, call: &str, row: &HeardRow, first: CqLine) {
        if self.cq_halves.len() >= CQ_PAIRS_KEPT && !self.cq_halves.contains_key(call) {
            let oldest = self.cq_halves.iter().min_by_key(|(_, h)| h.at).map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.cq_halves.remove(&k);
            }
        }
        let half = HalfCq { id: row.id, observed_utc: row.observed_utc.clone(), first, at: Instant::now() };
        self.cq_halves.insert(call.to_string(), half);
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
        match heard_station(store, callsign, &self.offsets) {
            Ok(station) => out.push(AppEvent::Heard { station }),
            Err(e) => tracing::warn!("could not read the heard list: {e}"),
        }
        out
    }

    /// Store one observation (a single short insert), with the frequency
    /// and offset that were fresh as it arrived; the stored row, with its id.
    fn insert(&mut self, store: &mut Store, row: HeardRow, tuning: &Tuning) -> Option<HeardRow> {
        let freq = tuning.freq.as_ref().and_then(|f| Some((i64::try_from(f.value).ok()?, f)));
        let mut row = HeardRow {
            freq_hz: freq.map(|(hz, _)| hz),
            freq_source: freq.map(|(_, f)| f.source.clone()),
            freq_age_ms: freq.and_then(|(_, f)| f.age_ms).and_then(|a| i64::try_from(a).ok()),
            ..row
        };
        match store.record_heard(&row) {
            Ok(id) => row.id = id,
            Err(e) => {
                tracing::warn!("could not record a heard station: {e}");
                return None;
            }
        }
        self.offsets.set(&row.callsign, &row.observed_utc, tuning.offset.clone());
        Some(row)
    }

    /// Store one observation and publish it.
    fn record(&mut self, store: Option<&mut Store>, row: HeardRow, how: &str, tuning: &Tuning) -> Vec<AppEvent> {
        let Some(store) = store else { return Vec::new() };
        match self.insert(store, row, tuning) {
            Some(row) => self.heard(store, &row.callsign, how),
            None => Vec::new(),
        }
    }

    /// The heard-list update for `callsign`, with no activity line (its
    /// observation's line has already been written).
    fn updated(&self, store: &Store, callsign: &str) -> Vec<AppEvent> {
        match heard_station(store, callsign, &self.offsets) {
            Ok(station) => vec![AppEvent::HeardUpdated { station }],
            Err(e) => {
                tracing::warn!("could not read the heard list: {e}");
                Vec::new()
            }
        }
    }

    /// One observation from the modem's control stream. Heard rows need the
    /// station's store; without one they are dropped.
    pub fn observation(&mut self, store: Option<&mut Store>, o: Observation, tuning: &Tuning) -> Vec<AppEvent> {
        match o {
            // One CQ is one row, whichever of its two lines comes first (lab
            // Q3): the second line completes the row the first one made.
            Observation::Cq { source, bw } => {
                let call = match self.callsign(&source, "CQ") {
                    Ok(c) => c,
                    Err(evs) => return evs,
                };
                let Some(store) = store else { return Vec::new() };
                if let Some(half) = self.take_half(&call, CqLine::Monitor) {
                    // The MONITOR line's row keeps its SNR and gains the
                    // bandwidth if it had none, and the offset of the frame
                    // Mercury's own decoder heard. Its line is already logged.
                    if let Some(bw) = bandwidth(&bw) {
                        if let Err(e) = store.fill_heard_bandwidth(half.id, &bw) {
                            tracing::warn!("could not complete a monitored CQ: {e}");
                        }
                    }
                    self.offsets.set(&call, &half.observed_utc, tuning.offset.clone());
                    return self.updated(store, &call);
                }
                let row = HeardRow { bandwidth: bandwidth(&bw), ..row(&call, "cq") };
                let Some(row) = self.insert(store, row, tuning) else { return Vec::new() };
                self.keep_half(&call, &row, CqLine::Frame);
                self.heard(store, &call, "CQ")
            }
            Observation::MonitorCq(m) => {
                let call = match self.callsign(&m.from, "monitored CQ") {
                    Ok(c) => c,
                    Err(evs) => return evs,
                };
                let Some(store) = store else { return Vec::new() };
                let how = format!("CQ, SNR {:.1} dB", m.snr_db);
                if let Some(half) = self.take_half(&call, CqLine::Frame) {
                    return match store.set_heard_snr(half.id, m.snr_db, MONITOR_SNR) {
                        Ok(()) => self.updated(store, &call),
                        Err(e) => {
                            tracing::warn!("could not record a monitored CQ: {e}");
                            Vec::new()
                        }
                    };
                }
                let row = HeardRow {
                    bandwidth: bandwidth(&m.bw),
                    snr_db: Some(m.snr_db),
                    snr_source: Some(MONITOR_SNR.into()),
                    ..row(&call, "cq")
                };
                // Mercury's offset is never a monitor decoder's.
                let tuning = Tuning { offset: None, ..tuning.clone() };
                let Some(row) = self.insert(store, row, &tuning) else { return Vec::new() };
                self.keep_half(&call, &row, CqLine::Monitor);
                self.heard(store, &call, &how)
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
            Observation::Unknown(_) | Observation::Snr(_) | Observation::MonitorBroadcast(_) => Vec::new(),
        }
    }

    /// One KISS frame from the broadcast port (03 §6).
    pub fn frame(&mut self, store: Option<&mut Store>, f: KissFrame, tuning: &Tuning) -> Vec<AppEvent> {
        if f.cmd != CMD_DATA_STD {
            return self.unsupported(format!("broadcast frame with KISS command 0x{:02x}", f.cmd));
        }
        match beacon::classify(&f.payload) {
            // A kind 2 beacon is heard by its kind 1 fields here; its TLV
            // fields come in with the extended beacon (M3c Task 11).
            Received::Beacon(b) => self.beacon(store, b, &[], tuning),
            Received::Extended(e) => self.beacon(store, e.beacon, &e.tlvs, tuning),
            // Heard traffic from its sender, whoever it is for; the
            // addressee's card is the service's (M3c B5).
            Received::Qsy(q) => {
                let call = match self.callsign(&q.from, "QSY suggestion") {
                    Ok(c) => c,
                    Err(evs) => return evs,
                };
                let mut out =
                    vec![self.note("broadcast", format!("QSY suggestion from {} to {}", shown(&q.from), shown(&q.to)))];
                out.extend(self.record(store, row(&call, "beacon"), "QSY suggestion", tuning));
                out
            }
            Received::Unsupported(kind) => self.unsupported(format!("MCB/1 kind {kind}, unknown to this version")),
            Received::Malformed(e) => self.malformed(format!("malformed MCB/1 beacon: {e}")),
            Received::Foreign => {
                self.unsupported(format!("another application's broadcast ({} bytes)", f.payload.len()))
            }
        }
    }

    /// A kind 1 or kind 2 beacon: heard by its kind 1 fields, with the
    /// kind 2 fields kept alongside.
    fn beacon(
        &mut self,
        store: Option<&mut Store>,
        b: beacon::Beacon,
        tlvs: &[beacon::Tlv],
        tuning: &Tuning,
    ) -> Vec<AppEvent> {
        let call = match self.callsign(&b.callsign, "beacon") {
            Ok(c) => c,
            Err(evs) => return evs,
        };
        let row = HeardRow { grid: b.grid, caps: Some(i64::from(b.caps)), ..row(&call, "beacon") };
        let Some(store) = store else { return Vec::new() };
        match self.insert(store, row, tuning) {
            Some(row) => {
                self.offsets.set_fields(&row.callsign, &row.observed_utc, tlvs);
                self.heard(store, &row.callsign, "beacon")
            }
            None => Vec::new(),
        }
    }

    /// A negotiated session with `peer`.
    pub fn session(&mut self, store: Option<&mut Store>, peer: &str, tuning: &Tuning) -> Vec<AppEvent> {
        match self.callsign(peer, "session") {
            Ok(call) => self.record(store, row(&call, "session"), "session", tuning),
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
