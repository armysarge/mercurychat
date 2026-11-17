//! The waterfall feed (OBS-02, M3a Task 12): the core keeps only the latest
//! valid spectrum from Mercury's telemetry; while the window shows the
//! spectrum or waterfall, this forwards it at most once every 200 ms (5 frames
//! a second) on its own event channel, never on the core's event stream. When
//! the panel is hidden nothing is forwarded. `None` (no telemetry, or it was
//! lost) is forwarded too, so the panel stops drawing instead of showing a
//! stale picture.

use mercury_adapter::telemetry::Spectrum;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

/// The event channel the spectrum goes out on.
pub const CHANNEL: &str = "mc://spectrum";
/// At most one frame per this long (5 FPS, 03 §4 / 04 §7).
pub const MIN_INTERVAL: Duration = Duration::from_millis(200);

/// One spectrum for the window: `fft_size` dB bins spanning 0 to
/// `sample_rate / 2` Hz (Mercury's FreeDV modem statistics).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub fft_size: u16,
    pub sample_rate: u16,
    pub bins: Vec<f32>,
}

impl From<&Spectrum> for Frame {
    fn from(s: &Spectrum) -> Frame {
        Frame { fft_size: s.fft_size, sample_rate: s.sample_rate, bins: s.bins.clone() }
    }
}

/// Forward the latest spectrum to `sink` while `visible` is true, no more
/// often than `MIN_INTERVAL`; frames that arrive in between are skipped (only
/// the newest counts). Returns when either sender is dropped.
pub async fn forward(
    mut spectra: watch::Receiver<Option<Arc<Spectrum>>>,
    mut visible: watch::Receiver<bool>,
    mut sink: impl FnMut(Option<Frame>),
) {
    loop {
        if visible.wait_for(|v| *v).await.is_err() {
            return;
        }
        // Shown: the current picture at once, then each new one, paced.
        let mut next = Instant::now();
        spectra.mark_changed();
        loop {
            tokio::select! {
                changed = visible.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    if !*visible.borrow_and_update() {
                        break;
                    }
                }
                changed = spectra.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    tokio::time::sleep_until(next).await;
                    if !*visible.borrow() {
                        break;
                    }
                    let frame = spectra.borrow_and_update().as_deref().map(Frame::from);
                    sink(frame);
                    next = Instant::now() + MIN_INTERVAL;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// What the sink saw: when, and which frame.
    type Seen = Arc<Mutex<Vec<(Instant, Option<Frame>)>>>;

    fn spectrum(level: f32) -> Option<Arc<Spectrum>> {
        Some(Arc::new(Spectrum { fft_size: 64, sample_rate: 8000, bins: vec![level; 64] }))
    }

    #[tokio::test(start_paused = true)]
    async fn spectrum_forwarding_is_capped_at_5_fps_and_stops_when_hidden() {
        let (feed, spectra) = watch::channel(None);
        let (show, visible) = watch::channel(false);
        let got: Seen = Arc::default();
        let sink = got.clone();
        let task = tokio::spawn(forward(spectra, visible, move |f| sink.lock().unwrap().push((Instant::now(), f))));
        let count = || got.lock().unwrap().len();

        // Hidden: Mercury's 20 frames a second are not forwarded at all.
        for i in 0..20 {
            feed.send_replace(spectrum(-100.0 + i as f32));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(count(), 0, "nothing is forwarded while hidden");

        // Shown for 2 s at 20 frames a second: at most 5 a second, the newest each time.
        show.send_replace(true);
        for i in 0..40 {
            feed.send_replace(spectrum(-60.0 + i as f32));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let shown = got.lock().unwrap().clone();
        assert!((9..=11).contains(&shown.len()), "{} frames in 2 s", shown.len());
        for pair in shown.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= MIN_INTERVAL, "frames {:?} apart", pair[1].0 - pair[0].0);
        }
        let last = shown.last().unwrap().1.as_ref().unwrap();
        assert!(last.bins[0] >= -60.0 + 35.0, "the newest frame is forwarded, not a stale one: {}", last.bins[0]);

        // Hidden again: forwarding stops.
        show.send_replace(false);
        tokio::time::sleep(Duration::from_millis(1)).await;
        let at_hide = count();
        for _ in 0..20 {
            feed.send_replace(spectrum(-50.0));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(count(), at_hide, "nothing is forwarded after hiding");

        // Telemetry lost while shown: `None` goes out, so the panel stops drawing.
        show.send_replace(true);
        tokio::time::sleep(Duration::from_millis(250)).await;
        feed.send_replace(None);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(got.lock().unwrap().last().unwrap().1, None);

        drop(feed);
        tokio::time::timeout(Duration::from_secs(1), task).await.expect("forward ends with its feed").unwrap();
    }
}
