//! Pacing file chunks by airtime (M4 C4): keep about 12 s of file data in
//! the modem queue, measured from how fast `BUFFER` drains, and never less
//! than one chunk. Chat and receipts go before the next chunk anyway; this
//! bounds how long they can wait behind what is already queued.

use mercury_adapter::Bandwidth;
use std::time::{Duration, Instant};

/// File data allowed in the modem queue, as airtime.
pub const QUEUE_AIRTIME: Duration = Duration::from_secs(12);
/// A chunk is about this much airtime.
const CHUNK_AIRTIME: f64 = 4.0;
/// The smallest chunk (spec C4: BW500 starts at 256 bytes, one outstanding).
const MIN_CHUNK: usize = 256;
/// Weight of a new drain measurement.
const SMOOTHING: f64 = 0.3;

/// The bandwidth's conservative throughput before anything is measured,
/// in bytes per second (M0: BW500 about 155 bit/s).
pub fn nominal_rate(bandwidth: Bandwidth) -> f64 {
    match bandwidth {
        Bandwidth::Bw500 => 19.0,
        Bandwidth::Bw2300 | Bandwidth::Bw2750 => 115.0,
    }
}

#[derive(Debug, Clone)]
pub struct Pacer {
    nominal: f64,
    max_chunk: usize,
    measured: Option<f64>,
    last: Option<(u64, Instant)>,
}

impl Pacer {
    pub fn new(nominal_bps: f64, max_chunk: usize) -> Pacer {
        Pacer { nominal: nominal_bps, max_chunk, measured: None, last: None }
    }

    /// A `BUFFER` reading. Only decreases count: they are the modem sending.
    pub fn observe_buffer(&mut self, queued: u64, now: Instant) {
        if let Some((prev, at)) = self.last {
            let dt = now.saturating_duration_since(at).as_secs_f64();
            if queued < prev && dt > 0.0 {
                let sample = (prev - queued) as f64 / dt;
                self.measured = Some(match self.measured {
                    Some(r) => r + SMOOTHING * (sample - r),
                    None => sample,
                });
            }
        }
        // The clock runs from the last change: a repeated value (a poll
        // answer) is not a new starting point.
        if self.last.is_none_or(|(prev, _)| prev != queued) {
            self.last = Some((queued, now));
        }
    }

    /// Bytes per second, and whether it was measured.
    pub fn rate(&self) -> (f64, bool) {
        match self.measured {
            Some(r) => (r, true),
            None => (self.nominal, false),
        }
    }

    /// About `CHUNK_AIRTIME` of data, a multiple of 64, never under one
    /// minimum chunk nor over what the peer agreed.
    pub fn chunk_size(&self) -> usize {
        let want = ((self.rate().0 * CHUNK_AIRTIME) as usize) / 64 * 64;
        want.clamp(MIN_CHUNK.min(self.max_chunk), self.max_chunk.max(1))
    }

    /// May another chunk go to the modem with `queued` bytes already there?
    pub fn may_send(&self, queued: u64) -> bool {
        let window = self.rate().0 * QUEUE_AIRTIME.as_secs_f64();
        queued == 0 || (queued + self.chunk_size() as u64) as f64 <= window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nominal_rates() {
        assert_eq!(nominal_rate(Bandwidth::Bw500), 19.0);
        assert_eq!(nominal_rate(Bandwidth::Bw2300), 115.0);
        assert_eq!(nominal_rate(Bandwidth::Bw2750), 115.0);
    }

    #[test]
    fn bw500_starts_at_256_one_chunk() {
        let p = Pacer::new(nominal_rate(Bandwidth::Bw500), 1024);
        assert_eq!(p.rate(), (19.0, false));
        assert_eq!(p.chunk_size(), 256);
        assert!(p.may_send(0));
        assert!(!p.may_send(1), "one chunk outstanding at BW500");
        assert!(!p.may_send(256));
    }

    #[test]
    fn measured_rate_grows_chunks() {
        let mut p = Pacer::new(19.0, 1024);
        let t = Instant::now();
        // 2000 bytes drain in 10 s, then 2000 more: 200 B/s.
        p.observe_buffer(4000, t);
        p.observe_buffer(2000, t + Duration::from_secs(10));
        p.observe_buffer(0, t + Duration::from_secs(20));
        let (r, measured) = p.rate();
        assert!(measured);
        assert!(r > 100.0 && r <= 200.0, "{r}");
        assert!(p.chunk_size() > 256 && p.chunk_size() <= 1024, "{}", p.chunk_size());
        assert_eq!(p.chunk_size() % 64, 0);
        assert!(p.may_send(p.chunk_size() as u64), "more than one chunk fits in 12 s at this rate");
    }

    #[test]
    fn writes_are_not_drain() {
        let mut p = Pacer::new(19.0, 1024);
        let t = Instant::now();
        p.observe_buffer(0, t);
        p.observe_buffer(500, t + Duration::from_secs(1));
        assert_eq!(p.rate(), (19.0, false), "a rise is our own write");
    }

    #[test]
    fn repeated_reading_keeps_the_clock() {
        // A poll answer repeats the last value just before the next drop:
        // the drop is timed from the last change, not from the repeat.
        let mut p = Pacer::new(19.0, 1024);
        let t = Instant::now();
        p.observe_buffer(100, t);
        p.observe_buffer(98, t + Duration::from_millis(100));
        p.observe_buffer(98, t + Duration::from_millis(190));
        p.observe_buffer(96, t + Duration::from_millis(200));
        let (r, _) = p.rate();
        assert!(r < 25.0, "2 bytes per 100 ms is 20 B/s, not {r}");
    }

    #[test]
    fn never_less_than_one_chunk() {
        let mut p = Pacer::new(19.0, 1024);
        let t = Instant::now();
        p.observe_buffer(1000, t);
        p.observe_buffer(999, t + Duration::from_secs(1000));
        assert!(p.rate().0 < 1.0);
        assert_eq!(p.chunk_size(), 256);
        assert!(p.may_send(0), "an empty queue always takes one chunk");
        // A peer that agreed a smaller chunk caps it.
        assert_eq!(Pacer::new(19.0, 128).chunk_size(), 128);
    }

    #[test]
    fn queued_airtime_never_over_12s() {
        // A modem draining 19 B/s, 64 KiB to send, 100 ms steps.
        let rate = 19.0;
        let mut p = Pacer::new(nominal_rate(Bandwidth::Bw500), 1024);
        let t0 = Instant::now();
        let (mut queued, mut left, mut drained) = (0.0f64, 65_536u64, 0.0f64);
        let mut step = 0u64;
        while left > 0 || queued > 0.0 {
            let now = t0 + Duration::from_millis(100 * step);
            p.observe_buffer(queued as u64, now);
            if left > 0 && p.may_send(queued as u64) {
                let c = (p.chunk_size() as u64).min(left);
                queued += c as f64;
                left -= c;
                p.observe_buffer(queued as u64, now);
                let bound = rate * 12.0 + p.chunk_size() as f64;
                assert!(queued <= bound, "queued {queued} over 12 s + one chunk ({bound}) at step {step}");
            }
            let d = (rate * 0.1).min(queued);
            queued -= d;
            drained += d;
            step += 1;
            assert!(step < 100_000);
        }
        assert!(drained >= 65_536.0 - 1.0);
    }
}
