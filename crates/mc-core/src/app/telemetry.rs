//! Telemetry as the UI sees it (OBS-01, OBS-02): local SNR, TX/RX mode,
//! queued bytes, app-acknowledged throughput, audio health, the radio's
//! frequency with its age, and the listen mode. Mercury's telemetry
//! WebSocket is optional: without it (or once it is lost) every reading it
//! supplied is `None`, shown as unavailable, never a stale value or a 0.
//! Losing it never touches the TNC connection or a session.
//!
//! The actor owns one [`Telemetry`]. It also supplies the frequency and
//! offset a heard station is recorded with ([`Tuning`]), only while fresh.

use super::broadcast::ListenMode;
use super::chat::ChatMessage;
use super::discovery::Reading;
use mc_storage::now_utc;
use mercury_adapter::telemetry::{Status, TelemetryLink, TelemetryMsg};
use serde::Serialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::Instant;
use ts_rs::TS;

/// The provenance of anything Mercury's telemetry WebSocket supplied.
pub const TELEMETRY_SOURCE: &str = "mercury telemetry";
/// The provenance of an SNR from the control connection's `SN` line.
pub const CONTROL_SOURCE: &str = "modem control";
/// A heard station takes the radio's frequency only if it is no older than this.
pub const FREQ_FRESH_MS: u64 = 10_000;
/// A heard station takes the carrier offset only if it is younger than this
/// when its frame arrives (the offset is of the last frame decoded).
pub const OFFSET_FRESH_MS: u64 = 2_000;
/// At most one telemetry event per this long (2 per second).
pub const EVENT_GAP: Duration = Duration::from_millis(500);

/// The telemetry WebSocket's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum TelemetryLinkView {
    /// The profile names no telemetry address (or the modem is a simulator).
    Unconfigured,
    Connecting,
    Up,
    /// Lost since `since_utc`; reconnecting by itself.
    Lost {
        since_utc: String,
        reason: String,
    },
}

/// Mercury's audio health. `ok` false with a reason while a device fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioView {
    pub ok: bool,
    pub error: Option<String>,
}

/// Everything the operator sees about the modem's link quality (OBS-01).
/// `None` is unavailable and is shown so, never as 0.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TelemetryView {
    pub link: TelemetryLinkView,
    /// Local SNR: from telemetry, else from the control connection's `SN`.
    pub snr_db: Option<Reading<f32>>,
    /// Mercury's ARQ payload modes (ours, and the peer's).
    pub tx_mode: Option<String>,
    pub rx_mode: Option<String>,
    /// What the modem holds unsent (`BUFFER`).
    #[ts(type = "number | null")]
    pub queued_bytes: Option<u64>,
    /// Bytes of messages the peer acknowledged this session, per second
    /// since it started; `None` before the first receipt.
    pub acked_bps: Option<f32>,
    pub audio: Option<AudioView>,
    /// The radio's dial frequency (Hz) as Mercury last read it, with its age.
    #[ts(as = "Option<Reading<f64>>")]
    pub frequency: Option<Reading<u64>>,
    /// The amateur band `frequency` falls in (e.g. `40m`); `None` with no
    /// frequency or outside every band (never guessed).
    pub band: Option<String>,
    /// The broadcast plane's listen mode (`MODE` index) as telemetry reports it.
    pub listen_mode: Option<u8>,
}

// Every float in a view is finite (parsed or computed so), so equality is total.
impl Eq for TelemetryView {}

/// The frequency and carrier offset to record with a heard station, each
/// only if fresh at the moment the frame or line arrived.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tuning {
    pub freq: Option<Reading<u64>>,
    pub offset: Option<Reading<f32>>,
}

/// The last status, and when this station received it.
struct Latest {
    status: Status,
    at: Instant,
    at_utc: String,
}

/// Messages acknowledged in the session now up.
struct Throughput {
    since: Instant,
    bytes: u64,
    counted: HashSet<String>,
}

/// The actor's telemetry state, and the throttle on its events.
pub struct Telemetry {
    link: TelemetryLinkView,
    /// Only while the link is up.
    latest: Option<Latest>,
    control_snr: Option<Reading<f32>>,
    queued: Option<u64>,
    session: Option<Throughput>,
    dirty: bool,
    last_emit: Option<Instant>,
}

impl Default for Telemetry {
    fn default() -> Self {
        Telemetry {
            link: TelemetryLinkView::Unconfigured,
            latest: None,
            control_snr: None,
            queued: None,
            session: None,
            dirty: false,
            last_emit: None,
        }
    }
}

/// What of a status the view shows (ages and times aside).
type Material<'a> =
    (Option<f32>, Option<&'a str>, Option<&'a str>, Option<bool>, Option<&'a str>, Option<u64>, Option<i32>);

fn material(s: &Status) -> Material<'_> {
    (
        s.snr,
        s.arq_tx_mode.as_deref(),
        s.arq_rx_mode.as_deref(),
        s.audio_ok,
        s.audio_error.as_deref(),
        s.radio_frequency_hz,
        s.listen_mode,
    )
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl Telemetry {
    /// A new modem: everything from the last one is forgotten.
    /// `configured`: the profile names a telemetry address.
    /// Published only if that changes what the operator sees.
    pub fn reset(&mut self, configured: bool) {
        let now = Instant::now();
        let before = self.view(now);
        let (dirty, last_emit) = (self.dirty, self.last_emit);
        *self = Telemetry {
            link: if configured { TelemetryLinkView::Connecting } else { TelemetryLinkView::Unconfigured },
            last_emit,
            ..Telemetry::default()
        };
        self.dirty = dirty || self.view(now) != before;
    }

    /// One message from the telemetry client. Returns a line for the
    /// activity log when the link came up or was lost.
    pub fn on_msg(&mut self, msg: TelemetryMsg, now: Instant) -> Option<String> {
        match msg {
            TelemetryMsg::Link(TelemetryLink::Connecting) => {
                self.set_link(TelemetryLinkView::Connecting);
                None
            }
            TelemetryMsg::Link(TelemetryLink::Up) => {
                self.set_link(TelemetryLinkView::Up);
                Some("telemetry connected".into())
            }
            TelemetryMsg::Link(TelemetryLink::Lost { reason }) => {
                let line = format!("telemetry lost: {reason}; its readings are unavailable until it reconnects");
                self.set_link(TelemetryLinkView::Lost { since_utc: now_utc(), reason });
                Some(line)
            }
            // The actor reads these itself (M3b): nothing for the view.
            TelemetryMsg::RadioList(_) | TelemetryMsg::Audio(_) | TelemetryMsg::CommandFailed { .. } => None,
            TelemetryMsg::Status(status) => {
                // A status only counts on a link that is up.
                if self.link != TelemetryLinkView::Up {
                    return None;
                }
                if self.latest.as_ref().is_none_or(|l| material(&l.status) != material(&status)) {
                    self.dirty = true;
                }
                self.latest = Some(Latest { status, at: now, at_utc: now_utc() });
                None
            }
        }
    }

    /// The modem task ended in a fault, so its telemetry client is gone
    /// (or never started, when attach failed). Any configured link (connecting or up) becomes lost here, with every
    /// reading cleared and the gate's live listen mode re-synced (ruling P2:
    /// back to the attach value). This rests on the awaited Faulted report,
    /// so it holds even when the dying task's own Lost (a `try_send`) is
    /// dropped on a full report channel.
    pub fn modem_faulted(&mut self, listen: &mut ListenMode) {
        if matches!(self.link, TelemetryLinkView::Connecting | TelemetryLinkView::Up) {
            let reason = "the modem connection ended".to_string();
            self.set_link(TelemetryLinkView::Lost { since_utc: now_utc(), reason });
        }
        listen.set_live(self.live_listen_mode());
    }

    fn set_link(&mut self, link: TelemetryLinkView) {
        if link != TelemetryLinkView::Up {
            self.latest = None; // nothing stale stays on show
        }
        if self.link != link {
            self.link = link;
            self.dirty = true;
        }
    }

    /// The listen mode telemetry reports right now, if any (ruling P2).
    pub fn live_listen_mode(&self) -> Option<u8> {
        self.latest.as_ref()?.status.listen_mode.and_then(|m| u8::try_from(m).ok())
    }

    /// An `SN` line on the control connection.
    pub fn on_control_snr(&mut self, snr_db: f32) {
        if self.control_snr.as_ref().map(|r| r.value) != Some(snr_db) {
            self.dirty = true;
        }
        self.control_snr =
            Some(Reading { value: snr_db, source: CONTROL_SOURCE.into(), at_utc: now_utc(), age_ms: None });
    }

    /// A `BUFFER` line: what the modem holds unsent.
    pub fn on_buffer(&mut self, bytes: u64) {
        if self.queued != Some(bytes) {
            self.queued = Some(bytes);
            self.dirty = true;
        }
    }

    pub fn session_started(&mut self, now: Instant) {
        self.session = Some(Throughput { since: now, bytes: 0, counted: HashSet::new() });
        self.dirty = true;
    }

    pub fn session_ended(&mut self) {
        if self.session.take().is_some() {
            self.dirty = true;
        }
    }

    /// A message changed state: an outgoing one delivered during the
    /// session counts toward its throughput, once.
    pub fn on_message(&mut self, m: &ChatMessage) {
        let Some(s) = self.session.as_mut() else { return };
        if m.direction == "out" && m.state == "delivered" && s.counted.insert(m.id.clone()) {
            s.bytes += m.body.len() as u64;
            self.dirty = true;
        }
    }

    pub fn view(&self, now: Instant) -> TelemetryView {
        let st = self.latest.as_ref();
        let status = st.map(|l| &l.status);
        // The SNR is of the last frame Mercury decoded: its age is that
        // decode's (`freq_offset_age_ms`), carried forward from receipt.
        let snr_db = st
            .and_then(|l| {
                let value = l.status.snr?;
                let age_ms = l.status.freq_offset_age_ms.map(|a| a.saturating_add(ms(now - l.at)));
                Some(Reading { value, source: TELEMETRY_SOURCE.into(), at_utc: l.at_utc.clone(), age_ms })
            })
            .or_else(|| self.control_snr.clone());
        let frequency = st.and_then(|l| {
            let value = l.status.radio_frequency_hz?;
            let age_ms = l.status.radio_frequency_age_ms.map(|a| a.saturating_add(ms(now - l.at)));
            Some(Reading { value, source: TELEMETRY_SOURCE.into(), at_utc: l.at_utc.clone(), age_ms })
        });
        let acked_bps = self.session.as_ref().filter(|s| !s.counted.is_empty()).map(|s| {
            // At least a second: a receipt just after the start is not a burst.
            let secs = (now - s.since).as_secs_f32().max(1.0);
            s.bytes as f32 / secs
        });
        TelemetryView {
            link: self.link.clone(),
            snr_db,
            tx_mode: status.and_then(|s| s.arq_tx_mode.clone()),
            rx_mode: status.and_then(|s| s.arq_rx_mode.clone()),
            queued_bytes: self.queued,
            acked_bps,
            audio: status.and_then(|s| Some(AudioView { ok: s.audio_ok?, error: s.audio_error.clone() })),
            band: frequency.as_ref().and_then(|f| crate::band::band_for(f.value)).map(String::from),
            frequency,
            listen_mode: self.live_listen_mode(),
        }
    }

    /// The frequency and offset a heard station arriving `now` is recorded
    /// with: the frequency if no older than 10 s, the offset if younger than
    /// 2 s. Unknown ages are never fresh.
    pub fn tuning(&self, now: Instant) -> Tuning {
        let Some(l) = self.latest.as_ref() else { return Tuning::default() };
        let since = ms(now - l.at);
        let s = &l.status;
        let freq = match (s.radio_frequency_hz, s.radio_frequency_age_ms) {
            (Some(value), Some(age)) if age.saturating_add(since) <= FREQ_FRESH_MS => Some(Reading {
                value,
                source: TELEMETRY_SOURCE.into(),
                at_utc: now_utc(),
                age_ms: Some(age.saturating_add(since)),
            }),
            _ => None,
        };
        let offset = match (s.freq_offset_hz, s.freq_offset_age_ms) {
            (Some(value), Some(age)) if age.saturating_add(since) < OFFSET_FRESH_MS => Some(Reading {
                value,
                source: TELEMETRY_SOURCE.into(),
                at_utc: now_utc(),
                age_ms: Some(age.saturating_add(since)),
            }),
            _ => None,
        };
        Tuning { freq, offset }
    }

    /// When the actor should next publish, if something changed.
    pub fn wake(&self) -> Option<Instant> {
        if !self.dirty {
            return None;
        }
        Some(self.last_emit.map_or_else(Instant::now, |t| t + EVENT_GAP))
    }

    /// The view to publish now, if something changed and the last event was
    /// at least [`EVENT_GAP`] ago.
    pub fn take_event(&mut self, now: Instant) -> Option<TelemetryView> {
        if !self.dirty || self.last_emit.is_some_and(|t| now < t + EVENT_GAP) {
            return None;
        }
        self.dirty = false;
        self.last_emit = Some(now);
        Some(self.view(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(json: &str) -> TelemetryMsg {
        TelemetryMsg::Status(mercury_adapter::telemetry::parse_text(json).unwrap().unwrap())
    }

    fn up(t: &mut Telemetry, now: Instant) {
        t.reset(true);
        t.on_msg(TelemetryMsg::Link(TelemetryLink::Up), now);
    }

    #[tokio::test(start_paused = true)]
    async fn events_are_at_most_two_a_second_and_the_last_change_is_never_lost() {
        let mut t = Telemetry::default();
        let start = Instant::now();
        up(&mut t, start);
        let mut sent = Vec::new();
        // A status with a new SNR every 100 ms for 3 s.
        for i in 0..30u32 {
            let now = start + Duration::from_millis(100 * u64::from(i));
            t.on_msg(status(&format!(r#"{{"type":"status","snr":{i}.0}}"#)), now);
            if let Some(v) = t.take_event(now) {
                sent.push((now, v));
            }
        }
        assert!(sent.windows(2).all(|w| w[1].0 - w[0].0 >= EVENT_GAP), "{:?}", sent.iter().map(|s| s.0 - start));
        assert!(sent.len() <= 7, "{} events in 3 s", sent.len());
        // The last value still goes out, at the next allowed moment.
        let wake = t.wake().expect("a pending change");
        let last = t.take_event(wake).expect("published at the wake");
        assert_eq!(last.snr_db.map(|r| r.value), Some(29.0));
        assert_eq!(t.wake(), None, "nothing left to publish");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unchanged_status_publishes_nothing() {
        let mut t = Telemetry::default();
        let now = Instant::now();
        up(&mut t, now);
        t.on_msg(
            status(r#"{"type":"status","snr":5.0,"radio_frequency_hz":7100000,"radio_frequency_age_ms":10}"#),
            now,
        );
        assert!(t.take_event(now).is_some());
        let later = now + Duration::from_secs(2);
        t.on_msg(
            status(r#"{"type":"status","snr":5.0,"radio_frequency_hz":7100000,"radio_frequency_age_ms":900}"#),
            later,
        );
        assert_eq!(t.take_event(later), None, "only ages moved");
    }

    #[tokio::test(start_paused = true)]
    async fn lost_clears_every_telemetry_reading_and_the_live_listen_mode() {
        let mut t = Telemetry::default();
        let now = Instant::now();
        up(&mut t, now);
        t.on_control_snr(3.0);
        t.on_buffer(120);
        t.on_msg(
            status(
                r#"{"type":"status","snr":5.0,"arq_tx_mode":"DATAC3","audio_ok":true,"listen_mode":1,
                    "radio_frequency_hz":7100000,"radio_frequency_age_ms":10}"#,
            ),
            now,
        );
        assert_eq!(t.live_listen_mode(), Some(1));
        assert_eq!(t.view(now).snr_db.unwrap().source, TELEMETRY_SOURCE);
        t.on_msg(TelemetryMsg::Link(TelemetryLink::Lost { reason: "closed".into() }), now);
        let v = t.view(now);
        assert!(matches!(v.link, TelemetryLinkView::Lost { .. }));
        assert_eq!((v.tx_mode, v.audio, v.frequency, v.listen_mode), (None, None, None, None));
        assert_eq!(t.live_listen_mode(), None);
        assert_eq!(v.snr_db.unwrap().source, CONTROL_SOURCE);
        assert_eq!(v.queued_bytes, Some(120), "BUFFER is the control connection's, not telemetry's");
        assert_eq!(t.tuning(now), Tuning::default());
    }

    #[tokio::test(start_paused = true)]
    async fn a_client_that_never_started_is_not_left_connecting() {
        let mut t = Telemetry::default();
        t.reset(true);
        assert_eq!(t.view(Instant::now()).link, TelemetryLinkView::Connecting);
        let mut listen = ListenMode::default();
        t.modem_faulted(&mut listen);
        assert!(matches!(t.view(Instant::now()).link, TelemetryLinkView::Lost { .. }));
        // Unconfigured stays so.
        t.reset(false);
        t.modem_faulted(&mut listen);
        assert_eq!(t.view(Instant::now()).link, TelemetryLinkView::Unconfigured);
    }

    /// Fix round 1: the awaited Faulted report alone clears an Up link, its
    /// readings and the gate's live listen mode; the client's own Lost (a
    /// try_send from the dying task) may never arrive.
    #[tokio::test(start_paused = true)]
    async fn a_modem_fault_clears_an_up_link_and_the_live_listen_mode() {
        let mut t = Telemetry::default();
        let now = Instant::now();
        up(&mut t, now);
        t.on_msg(
            status(
                r#"{"type":"status","snr":5.0,"arq_tx_mode":"DATAC3","audio_ok":true,"listen_mode":200,
                    "radio_frequency_hz":7100000,"radio_frequency_age_ms":10,
                    "freq_offset_hz":2.0,"freq_offset_age_ms":10}"#,
            ),
            now,
        );
        let mut listen = ListenMode::default();
        listen.set_attach(Some(1));
        listen.set_live(t.live_listen_mode());
        assert_eq!(listen.current(), Some(200), "telemetry's live mode is preferred while up");
        t.take_event(now);

        t.modem_faulted(&mut listen);
        let v = t.view(now);
        assert!(matches!(v.link, TelemetryLinkView::Lost { .. }), "{:?}", v.link);
        assert_eq!((v.snr_db, v.tx_mode, v.audio, v.frequency, v.listen_mode), (None, None, None, None, None));
        assert_eq!(t.tuning(now), Tuning::default());
        assert_eq!(listen.current(), Some(1), "the gate falls back to the attach value");
        assert!(t.take_event(now + EVENT_GAP).is_some(), "the loss is published");
    }

    /// OBS-01 provenance (13F carry): the telemetry SNR carries the age of
    /// the decode it measures, aged from receipt like the frequency.
    #[tokio::test(start_paused = true)]
    async fn telemetry_snr_carries_its_decode_age() {
        let mut t = Telemetry::default();
        let now = Instant::now();
        up(&mut t, now);
        t.on_msg(status(r#"{"type":"status","snr":7.5,"freq_offset_hz":1.0,"freq_offset_age_ms":300}"#), now);
        let snr = t.view(now + Duration::from_millis(200)).snr_db.expect("an SNR after a decode");
        assert_eq!((snr.value, snr.source.as_str(), snr.age_ms), (7.5, TELEMETRY_SOURCE, Some(500)));
    }

    #[tokio::test(start_paused = true)]
    async fn tuning_ages_from_receipt() {
        let mut t = Telemetry::default();
        let now = Instant::now();
        up(&mut t, now);
        t.on_msg(
            status(
                r#"{"type":"status","radio_frequency_hz":7100000,"radio_frequency_age_ms":9000,
                    "freq_offset_hz":-3.5,"freq_offset_age_ms":1500}"#,
            ),
            now,
        );
        let fresh = t.tuning(now + Duration::from_millis(400));
        assert_eq!(fresh.freq.as_ref().map(|r| (r.value, r.age_ms)), Some((7_100_000, Some(9400))));
        assert_eq!(fresh.offset.as_ref().map(|r| (r.value, r.age_ms)), Some((-3.5, Some(1900))));
        let later = t.tuning(now + Duration::from_millis(1000));
        assert!(later.freq.is_some(), "10 s exactly is still fresh");
        assert_eq!(later.offset, None, "2.5 s is too old for the frame now arriving");
        assert_eq!(t.tuning(now + Duration::from_millis(1001)).freq, None);
    }
}
