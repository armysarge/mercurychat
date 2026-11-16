//! M3a Task 7: Mercury's telemetry WebSocket (OBS-01, OBS-02). The `status`
//! parse (absent is `None`, never 0), the binary spectrum decoder (03 §4) and
//! the client's reconnect schedule.

use mercury_adapter::fake_ws::FakeWs;
use mercury_adapter::telemetry::{
    self, Frame, Spectrum, SpectrumError, Status, TelemetryClient, TelemetryError, TelemetryLink, TelemetryMsg,
    MAX_TEXT_FRAME,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

fn status(json: &str) -> Status {
    telemetry::parse_text(json).expect("parses").expect("a status")
}

#[test]
fn status_fixture_parses_with_nulls_as_none() {
    // WEBSOCKET_API.md §5.1, verbatim.
    let st = status(include_str!("fixtures/ws/status.json"));
    assert_eq!(st.snr, Some(0.0), "a value Mercury sent is kept, even 0");
    assert_eq!(st.bitrate, Some(0));
    assert_eq!(st.direction.as_deref(), Some("rx"));
    assert_eq!(st.arq_tx_mode, None, "an empty mode is unavailable");
    assert_eq!(st.arq_rx_mode, None);
    assert_eq!(st.tx_gain_db, Some(0.0));
    assert_eq!(st.audio_ok, Some(true));
    assert_eq!(st.audio_error, None, "an empty reason is no reason");
    assert_eq!(st.radio_frequency_hz, None, "null is unavailable, never 0");
    assert_eq!(st.radio_frequency_age_ms, None);
    // Fields the fixture does not carry are unavailable, never a default.
    assert_eq!(st.freq_offset_hz, None);
    assert_eq!(st.freq_offset_age_ms, None);
    assert_eq!(st.listen_mode, None);
    assert_eq!(st.busy, None);

    // Nothing at all: every field None.
    assert_eq!(status(r#"{"type":"status"}"#), Status::default());

    let st = status(
        r#"{"type":"status","snr":7.5,"arq_tx_mode":"DATAC3","arq_rx_mode":"DATAC1",
            "radio_frequency_hz":14105000,"radio_frequency_age_ms":250,
            "freq_offset_hz":-12.5,"freq_offset_age_ms":40,
            "listen_mode":1,"listen_mode_name":"DATAC3","busy":"arq","audio_ok":false,
            "audio_error":"capture: the capture device delivers no audio (still trying it)"}"#,
    );
    assert_eq!(st.snr, Some(7.5));
    assert_eq!(st.arq_tx_mode.as_deref(), Some("DATAC3"));
    assert_eq!(st.arq_rx_mode.as_deref(), Some("DATAC1"));
    assert_eq!(st.radio_frequency_hz, Some(14_105_000));
    assert_eq!(st.radio_frequency_age_ms, Some(250));
    assert_eq!(st.freq_offset_hz, Some(-12.5));
    assert_eq!(st.freq_offset_age_ms, Some(40));
    assert_eq!(st.listen_mode, Some(1));
    assert_eq!(st.busy.as_deref(), Some("arq"));
    assert_eq!(st.audio_ok, Some(false));
    assert!(st.audio_error.as_deref().unwrap().starts_with("capture:"));

    // -1 is Mercury's "unknown" listen mode.
    assert_eq!(status(r#"{"type":"status","listen_mode":-1,"listen_mode_name":""}"#).listen_mode, None);
    // A field of the wrong type, or a non-finite number, is unavailable too.
    let st = status(r#"{"type":"status","snr":"high","bitrate":-5,"radio_frequency_hz":1.5e30}"#);
    assert_eq!((st.snr, st.bitrate, st.radio_frequency_hz), (None, None, None));

    // Other events are not a status; garbage is an error, never a status.
    assert_eq!(telemetry::parse_text(r#"{"type":"history","messages":[]}"#).unwrap(), None);
    assert_eq!(telemetry::parse_text(r#"{"type":"radio_list","list":[]}"#).unwrap(), None);
    assert!(telemetry::parse_text("not json").is_err());
    assert!(telemetry::parse_text("[1,2,3]").is_err());
    assert!(telemetry::parse_text(r#"{"snr":5}"#).is_err(), "no type");
}

#[test]
fn peer_snr_sentinel_is_ignored() {
    // The carousel's -99.9 sentinel is never a measurement, and a "valid"
    // peer_snr is not our SNR either: it is not parsed at all.
    let st = status(r#"{"type":"status","peer_snr":-99.9,"peer_snr_valid":false,"peer_hears_mode":""}"#);
    assert_eq!(st, Status::default());
    assert_eq!(st.snr, None);
    let st = status(r#"{"type":"status","peer_snr":12.5,"peer_snr_valid":true}"#);
    assert_eq!(st, Status::default());
    assert!(!format!("{st:?}").contains("99.9"));
}

/// A spectrum frame: LE magic, LE FFT size, LE sample rate, LE f32 bins.
fn frame(n: u16, rate: u16, bins: &[f32]) -> Vec<u8> {
    let mut b = 0x4D43_5259u32.to_le_bytes().to_vec();
    b.extend_from_slice(&n.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    for v in bins {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

fn ramp(n: usize) -> Vec<f32> {
    (0..n).map(|i| -120.0 + i as f32 * 0.25).collect()
}

#[test]
fn spectrum_rejects_wrong_length_nan_and_bad_n() {
    let good = frame(64, 8000, &ramp(64));
    let s = Spectrum::decode(&good).expect("a valid frame");
    assert_eq!((s.fft_size, s.sample_rate, s.bins.len()), (64, 8000, 64));
    assert_eq!(s.bins[3], -119.25);
    assert_eq!(Spectrum::decode(&frame(8192, 8000, &ramp(8192))).unwrap().bins.len(), 8192);

    // Length must be exactly 8 + 4*N.
    assert!(matches!(Spectrum::decode(&good[..good.len() - 1]), Err(SpectrumError::Length { .. })));
    let mut long = good.clone();
    long.extend_from_slice(&0f32.to_le_bytes());
    assert!(matches!(Spectrum::decode(&long), Err(SpectrumError::Length { .. })));
    // Shorter than a header.
    assert!(matches!(Spectrum::decode(&good[..7]), Err(SpectrumError::Short(7))));
    assert!(Spectrum::decode(&[]).is_err());
    // N: a power of two in 64..=8192.
    for n in [0u16, 1, 32, 100, 4095, 16384] {
        let f = frame(n, 8000, &ramp(n as usize));
        assert!(matches!(Spectrum::decode(&f), Err(SpectrumError::FftSize(m)) if m == n), "N={n}");
    }
    // Non-finite bins.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut bins = ramp(64);
        bins[17] = bad;
        assert!(matches!(Spectrum::decode(&frame(64, 8000, &bins)), Err(SpectrumError::NonFinite(17))), "{bad}");
    }
}

#[test]
fn spectrum_magic_is_little_endian() {
    let good = frame(256, 48000, &ramp(256));
    assert_eq!(&good[..4], &[0x59, 0x52, 0x43, 0x4D], "the wire bytes spell YRCM");
    assert_eq!(&good[4..6], &[0x00, 0x01], "FFT size 256, little-endian");
    let s = Spectrum::decode(&good).unwrap();
    assert_eq!((s.fft_size, s.sample_rate), (256, 48000));
    // The readable spelling, MCRY, is the big-endian order: refused.
    let mut be = good.clone();
    be[..4].copy_from_slice(&0x4D43_5259u32.to_be_bytes());
    assert_eq!(&be[..4], b"MCRY");
    assert!(matches!(Spectrum::decode(&be), Err(SpectrumError::Magic(_))));
    // A big-endian FFT size is a different N: refused by length.
    let mut swapped = good;
    swapped[4..6].copy_from_slice(&256u16.to_be_bytes());
    assert!(Spectrum::decode(&swapped).is_err());
}

async fn next(rx: &mut mpsc::Receiver<TelemetryMsg>, deadline: Instant) -> TelemetryMsg {
    tokio::time::timeout_at(deadline, rx.recv()).await.expect("telemetry in time").expect("client running")
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_text_frame_is_refused() {
    let big = format!(r#"{{"type":"status","snr":5.0,"audio_error":"{}"}}"#, "x".repeat(MAX_TEXT_FRAME));
    assert!(matches!(telemetry::parse_text(&big), Err(TelemetryError::TooLarge(n)) if n == big.len()));
    let just = format!(r#"{{"type":"status","audio_error":"{}"}}"#, "x".repeat(1000));
    assert!(telemetry::parse_text(&just).is_ok());

    // Over the wire: a 1 MiB frame never becomes a status, and the client
    // goes on to take the next good one.
    let ws = FakeWs::start().await;
    let (tx, mut rx) = mpsc::channel(64);
    let (spectrum, _keep) = watch::channel(None);
    let client = TelemetryClient::spawn(ws.url.clone(), tx, spectrum);
    let deadline = Instant::now() + Duration::from_secs(10);
    assert_eq!(next(&mut rx, deadline).await, TelemetryMsg::Link(TelemetryLink::Connecting));
    assert_eq!(next(&mut rx, deadline).await, TelemetryMsg::Link(TelemetryLink::Up));
    ws.send_text(format!(r#"{{"type":"status","snr":1.0,"audio_error":"{}"}}"#, "y".repeat(1024 * 1024)));
    ws.set_status(Some(r#"{"type":"status","snr":3.5}"#));
    loop {
        match next(&mut rx, deadline).await {
            TelemetryMsg::Status(st) => {
                assert_eq!(st.snr, Some(3.5), "only the good status is taken: {st:?}");
                break;
            }
            TelemetryMsg::Link(_) => continue, // it may drop and reconnect
        }
    }
    client.abort();
}

#[tokio::test(start_paused = true)]
async fn reconnect_follows_backoff() {
    // Every dial fails at once; the clock is paused, so the gaps are exact.
    let tries: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let seen = tries.clone();
    let dial = move || {
        let seen = seen.clone();
        async move {
            seen.lock().unwrap().push(Instant::now());
            Err::<telemetry::Conn, String>("connection refused".into())
        }
    };
    let (tx, mut rx) = mpsc::channel(64);
    let (spectrum, _keep) = watch::channel(None);
    let client = TelemetryClient::spawn_with(dial, tx, spectrum, |d| d);
    assert_eq!(rx.recv().await, Some(TelemetryMsg::Link(TelemetryLink::Connecting)));
    assert!(matches!(rx.recv().await, Some(TelemetryMsg::Link(TelemetryLink::Lost { .. }))));
    tokio::time::sleep(Duration::from_secs(1 + 2 + 4 + 8 + 15 + 15) + Duration::from_millis(500)).await;
    let gaps: Vec<u64> = tries.lock().unwrap().windows(2).map(|w| (w[1] - w[0]).as_millis() as u64).collect();
    assert_eq!(gaps, [1000, 2000, 4000, 8000, 15000, 15000], "1, 2, 4, 8, then 15 s at most");
    // Still lost: said once, not once per try.
    assert!(rx.try_recv().is_err(), "no Lost/Connecting flapping while it retries");
    client.abort();

    // The jitter applies to every wait, and never lengthens past the step.
    for step in telemetry::BACKOFF {
        for _ in 0..100 {
            let d = telemetry::jitter(step);
            assert!(d <= step && d >= step.mul_f64(0.8), "{d:?} for {step:?}");
        }
    }

    // A connection that delivered telemetry starts the schedule again at 1 s.
    let tries: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let seen = tries.clone();
    let dial = move || {
        let seen = seen.clone();
        async move {
            let n = {
                let mut t = seen.lock().unwrap();
                t.push(Instant::now());
                t.len()
            };
            if n == 3 {
                let good: telemetry::Conn =
                    Box::pin(futures_util::stream::iter([Ok(Frame::Text(r#"{"type":"status","snr":2.0}"#.into()))]));
                Ok(good)
            } else {
                Err("connection refused".into())
            }
        }
    };
    let (tx, mut rx) = mpsc::channel(64);
    let (spectrum, _keep) = watch::channel(None);
    let client = TelemetryClient::spawn_with(dial, tx, spectrum, |d| d);
    tokio::time::sleep(Duration::from_secs(1 + 2 + 1 + 2) + Duration::from_millis(500)).await;
    let gaps: Vec<u64> = tries.lock().unwrap().windows(2).map(|w| (w[1] - w[0]).as_millis() as u64).collect();
    assert_eq!(gaps, [1000, 2000, 1000, 2000]);
    let mut got = Vec::new();
    while let Ok(m) = rx.try_recv() {
        got.push(m);
    }
    assert!(
        matches!(
            got.as_slice(),
            [
                TelemetryMsg::Link(TelemetryLink::Connecting),
                TelemetryMsg::Link(TelemetryLink::Lost { .. }),
                TelemetryMsg::Link(TelemetryLink::Up),
                TelemetryMsg::Status(_),
                TelemetryMsg::Link(TelemetryLink::Lost { .. }),
            ]
        ),
        "{got:?}"
    );
    client.abort();
}
