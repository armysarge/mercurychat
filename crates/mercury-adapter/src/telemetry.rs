//! Mercury's telemetry WebSocket (WEBSOCKET_API.md; MercuryChat OBS-01,
//! OBS-02): the `status` event, the binary spectrum frame, and a client that
//! reconnects on its own. Telemetry is optional and read-only: the client
//! never sends a command, and losing it never touches the TNC connection.
//!
//! Everything here comes from another process and is bounded: text frames
//! over 64 KiB are refused, a status field that is absent, null or of the
//! wrong type is `None` (never a default 0), and a spectrum frame must match
//! 03 §4 exactly.

use futures_util::{Stream, StreamExt};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::AbortHandle;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

/// A text frame longer than this is refused unread.
pub const MAX_TEXT_FRAME: usize = 64 * 1024;
/// Any message longer than this closes the connection (the largest valid
/// frame, an 8192-bin spectrum, is 32776 bytes).
pub const MAX_WS_MESSAGE: usize = 128 * 1024;
/// The reconnect waits (03 §2): 1, 2, 4, 8, then 15 s at most, each jittered.
pub const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(15),
];
/// How long a connection attempt may take.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
/// Mercury publishes `status` every 500 ms: this long without a valid
/// status is a lost connection, whatever else arrives (garbage, other
/// events, spectrum frames), so a stale status is never kept looking live.
pub const SILENCE: Duration = Duration::from_secs(5);
/// The spectrum frame's magic, read little-endian (wire bytes `59 52 43 4D`).
pub const SPECTRUM_MAGIC: u32 = 0x4D43_5259;
/// The FFT sizes accepted: a power of two in this range.
pub const FFT_MIN: u16 = 64;
pub const FFT_MAX: u16 = 8192;

/// Mercury's `status` event (WEBSOCKET_API.md §5.1), as far as MercuryChat
/// reads it. Every field is optional: absent, null, of the wrong type or out
/// of range is `None`, never a default. `peer_snr` is deliberately not read:
/// it is a -99.9 sentinel, never a measurement (03 §4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// Local SNR, dB.
    pub snr: Option<f32>,
    /// Bits per second.
    pub bitrate: Option<u32>,
    /// `tx` or `rx`.
    pub direction: Option<String>,
    /// Local ARQ payload mode; an empty string is `None`.
    pub arq_tx_mode: Option<String>,
    /// The peer's payload mode; an empty string is `None`.
    pub arq_rx_mode: Option<String>,
    pub tx_gain_db: Option<f32>,
    pub audio_ok: Option<bool>,
    /// Why audio is not ok; an empty string is `None`.
    pub audio_error: Option<String>,
    /// Mercury's cached Hamlib frequency, and how old it was when sent.
    pub radio_frequency_hz: Option<u64>,
    pub radio_frequency_age_ms: Option<u64>,
    /// The carrier offset of the last decoded frame, and its age when sent.
    pub freq_offset_hz: Option<f32>,
    pub freq_offset_age_ms: Option<u64>,
    /// The broadcast plane's listen mode (`MODE` index); `-1` is `None`.
    pub listen_mode: Option<i32>,
    /// What holds the modem: `arq`, `broadcast`, or `""` when free.
    pub busy: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TelemetryError {
    #[error("a {0}-byte text frame is over the 64 KiB limit")]
    TooLarge(usize),
    #[error("not JSON: {0}")]
    Json(String),
    #[error("not a Mercury event (no type)")]
    NotAnEvent,
}

fn finite_f32(v: &serde_json::Value) -> Option<f32> {
    let x = v.as_f64()? as f32;
    x.is_finite().then_some(x)
}

/// A short token (a mode name): ASCII letters, digits, `_` or `-`, at most
/// 16. An empty one is `None` unless `empty_ok`.
fn token(v: &serde_json::Value, empty_ok: bool) -> Option<String> {
    let s = v.as_str()?;
    let ok = s.len() <= 16 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (ok && (empty_ok || !s.is_empty())).then(|| s.to_string())
}

/// Free text from Mercury, made safe to show: no control characters, at most
/// 256 characters, and an empty one is `None`.
fn text(v: &serde_json::Value) -> Option<String> {
    let s: String = v.as_str()?.chars().filter(|c| !c.is_control()).take(256).collect();
    (!s.is_empty()).then_some(s)
}

/// Read one text frame. `Ok(None)` for any event other than `status`.
pub fn parse_text(frame: &str) -> Result<Option<Status>, TelemetryError> {
    if frame.len() > MAX_TEXT_FRAME {
        return Err(TelemetryError::TooLarge(frame.len()));
    }
    let v: serde_json::Value = serde_json::from_str(frame).map_err(|e| TelemetryError::Json(e.to_string()))?;
    let obj = v.as_object().ok_or(TelemetryError::NotAnEvent)?;
    let kind = obj.get("type").and_then(|t| t.as_str()).ok_or(TelemetryError::NotAnEvent)?;
    if kind != "status" {
        return Ok(None);
    }
    let get = |k: &str| obj.get(k).filter(|v| !v.is_null());
    Ok(Some(Status {
        snr: get("snr").and_then(finite_f32),
        bitrate: get("bitrate").and_then(|v| v.as_u64()).and_then(|n| u32::try_from(n).ok()),
        direction: get("direction").and_then(|v| token(v, false)),
        arq_tx_mode: get("arq_tx_mode").and_then(|v| token(v, false)),
        arq_rx_mode: get("arq_rx_mode").and_then(|v| token(v, false)),
        tx_gain_db: get("tx_gain_db").and_then(finite_f32),
        audio_ok: get("audio_ok").and_then(|v| v.as_bool()),
        audio_error: get("audio_error").and_then(text),
        radio_frequency_hz: get("radio_frequency_hz").and_then(|v| v.as_u64()),
        radio_frequency_age_ms: get("radio_frequency_age_ms").and_then(|v| v.as_u64()),
        freq_offset_hz: get("freq_offset_hz").and_then(finite_f32),
        freq_offset_age_ms: get("freq_offset_age_ms").and_then(|v| v.as_u64()),
        listen_mode: get("listen_mode")
            .and_then(|v| v.as_i64())
            .and_then(|n| i32::try_from(n).ok())
            .filter(|n| *n >= 0),
        busy: get("busy").and_then(|v| token(v, true)),
    }))
}

/// One spectrum (waterfall line), validated (03 §4).
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    pub fft_size: u16,
    pub sample_rate: u16,
    /// `fft_size` finite dB values.
    pub bins: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpectrumError {
    #[error("{0} bytes is shorter than the 8-byte header")]
    Short(usize),
    #[error("magic 0x{0:08x} is not Mercury's spectrum")]
    Magic(u32),
    #[error("FFT size {0} is not a power of two in 64..=8192")]
    FftSize(u16),
    #[error("{len} bytes; an FFT size of {fft_size} needs exactly {expected}")]
    Length { len: usize, fft_size: u16, expected: usize },
    #[error("bin {0} is not a finite number")]
    NonFinite(usize),
}

impl Spectrum {
    /// Decode one binary frame: LE magic, LE FFT size N, LE sample rate,
    /// then N LE `f32` bins; exactly `8 + 4*N` bytes.
    pub fn decode(b: &[u8]) -> Result<Spectrum, SpectrumError> {
        if b.len() < 8 {
            return Err(SpectrumError::Short(b.len()));
        }
        let magic = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if magic != SPECTRUM_MAGIC {
            return Err(SpectrumError::Magic(magic));
        }
        let fft_size = u16::from_le_bytes([b[4], b[5]]);
        if !(FFT_MIN..=FFT_MAX).contains(&fft_size) || !fft_size.is_power_of_two() {
            return Err(SpectrumError::FftSize(fft_size));
        }
        let sample_rate = u16::from_le_bytes([b[6], b[7]]);
        let expected = 8 + 4 * usize::from(fft_size);
        if b.len() != expected {
            return Err(SpectrumError::Length { len: b.len(), fft_size, expected });
        }
        let mut bins = Vec::with_capacity(usize::from(fft_size));
        for (i, c) in b[8..].as_chunks::<4>().0.iter().enumerate() {
            let v = f32::from_le_bytes(*c);
            if !v.is_finite() {
                return Err(SpectrumError::NonFinite(i));
            }
            bins.push(v);
        }
        Ok(Spectrum { fft_size, sample_rate, bins })
    }

    /// The wire form of this spectrum (for fakes and tests).
    pub fn encode(&self) -> Vec<u8> {
        let mut b = SPECTRUM_MAGIC.to_le_bytes().to_vec();
        b.extend_from_slice(&self.fft_size.to_le_bytes());
        b.extend_from_slice(&self.sample_rate.to_le_bytes());
        for v in &self.bins {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// The telemetry connection as the client sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryLink {
    /// The first attempt is under way.
    Connecting,
    Up,
    /// Not connected; retried on the backoff. Said once per loss, not once
    /// per failed retry.
    Lost {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TelemetryMsg {
    Link(TelemetryLink),
    Status(Status),
}

/// One message read from a telemetry connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

/// An open telemetry connection: its frames, then its end (an error, or the
/// stream ending, is the connection lost).
pub type Conn = Pin<Box<dyn Stream<Item = Result<Frame, String>> + Send>>;

static JITTER_SEQ: AtomicU64 = AtomicU64::new(0);

/// A wait of `step` with jitter: between 80 % and 100 % of it, so the
/// 15 s ceiling is never exceeded.
pub fn jitter(step: Duration) -> Duration {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(JITTER_SEQ.fetch_add(1, Ordering::Relaxed));
    let unit = (h.finish() >> 11) as f64 / (1u64 << 53) as f64;
    step.mul_f64(0.8 + 0.2 * unit)
}

/// Open a plain `ws://` connection to Mercury, bounded in time and size.
async fn dial_ws(url: String) -> Result<Conn, String> {
    let cfg = WebSocketConfig::default().max_message_size(Some(MAX_WS_MESSAGE)).max_frame_size(Some(MAX_WS_MESSAGE));
    let connect = tokio_tungstenite::connect_async_with_config(url.as_str(), Some(cfg), false);
    let (ws, _) = match tokio::time::timeout(DIAL_TIMEOUT, connect).await {
        Err(_) => return Err(format!("no answer within {} s", DIAL_TIMEOUT.as_secs())),
        Ok(Err(e)) => return Err(e.to_string()),
        Ok(Ok(opened)) => opened,
    };
    // Pings are answered by the reader itself; nothing is ever sent.
    let frames = ws.filter_map(|m| async move {
        match m {
            Ok(Message::Text(t)) => Some(Ok(Frame::Text(t.as_str().to_string()))),
            Ok(Message::Binary(b)) => Some(Ok(Frame::Binary(b.to_vec()))),
            Ok(Message::Close(_)) => Some(Err("closed by Mercury".to_string())),
            Ok(_) => None,
            Err(e) => Some(Err(e.to_string())),
        }
    });
    Ok(Box::pin(frames))
}

/// The telemetry client: one task, which connects, reads, and reconnects on
/// [`BACKOFF`] until aborted. It never sends a command.
pub struct TelemetryClient;

impl TelemetryClient {
    /// Connect to Mercury's WebSocket at `url` (`ws://host:port/path`).
    /// Statuses and link changes go to `out`; each valid spectrum replaces
    /// the one in `spectrum`, which is cleared whenever the link is lost.
    pub fn spawn(
        url: String,
        out: mpsc::Sender<TelemetryMsg>,
        spectrum: watch::Sender<Option<Arc<Spectrum>>>,
    ) -> AbortHandle {
        Self::spawn_with(move || dial_ws(url.clone()), out, spectrum, jitter)
    }

    /// As [`TelemetryClient::spawn`], with the connection and the jitter
    /// supplied: tests use it to run the schedule on a paused clock.
    pub fn spawn_with<D, F>(
        dial: D,
        out: mpsc::Sender<TelemetryMsg>,
        spectrum: watch::Sender<Option<Arc<Spectrum>>>,
        jitter: impl FnMut(Duration) -> Duration + Send + 'static,
    ) -> AbortHandle
    where
        D: FnMut() -> F + Send + 'static,
        F: Future<Output = Result<Conn, String>> + Send + 'static,
    {
        tokio::spawn(run(dial, out, spectrum, jitter)).abort_handle()
    }
}

/// Where the client is, so each change is reported once.
#[derive(PartialEq)]
enum Said {
    Connecting,
    Up,
    Lost,
}

async fn run<D, F>(
    mut dial: D,
    out: mpsc::Sender<TelemetryMsg>,
    spectrum: watch::Sender<Option<Arc<Spectrum>>>,
    mut jitter: impl FnMut(Duration) -> Duration,
) where
    D: FnMut() -> F,
    F: Future<Output = Result<Conn, String>>,
{
    let mut step = 0usize;
    let mut said = Said::Connecting;
    if out.send(TelemetryMsg::Link(TelemetryLink::Connecting)).await.is_err() {
        return;
    }
    loop {
        let reason = match dial().await {
            Err(e) => e,
            Ok(mut conn) => {
                said = Said::Up;
                if out.send(TelemetryMsg::Link(TelemetryLink::Up)).await.is_err() {
                    return;
                }
                // Only a valid status moves this deadline on.
                let mut status_due = tokio::time::Instant::now() + SILENCE;
                let reason = loop {
                    match tokio::time::timeout_at(status_due, conn.next()).await {
                        Err(_) => break format!("no status from Mercury for {} s", SILENCE.as_secs()),
                        Ok(None) => break "closed by Mercury".to_string(),
                        Ok(Some(Err(e))) => break e,
                        // Garbage is dropped; only good telemetry counts.
                        Ok(Some(Ok(Frame::Text(t)))) => {
                            if let Ok(Some(st)) = parse_text(&t) {
                                step = 0;
                                status_due = tokio::time::Instant::now() + SILENCE;
                                if out.send(TelemetryMsg::Status(st)).await.is_err() {
                                    return;
                                }
                            }
                        }
                        Ok(Some(Ok(Frame::Binary(b)))) => {
                            if let Ok(s) = Spectrum::decode(&b) {
                                step = 0;
                                spectrum.send_replace(Some(Arc::new(s)));
                            }
                        }
                    }
                };
                spectrum.send_replace(None);
                reason
            }
        };
        if said != Said::Lost {
            said = Said::Lost;
            if out.send(TelemetryMsg::Link(TelemetryLink::Lost { reason })).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(jitter(BACKOFF[step])).await;
        step = (step + 1).min(BACKOFF.len() - 1);
    }
}
