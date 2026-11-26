//! Which S/N report belongs to which broadcast frame. Mercury reports the S/N
//! of a broadcast on the control port (`MONITOR ... BCAST_DATA`) and delivers
//! the frame on the broadcast port, naming no sender in the report: only the
//! arrival times tie them together. A frame gets an S/N only when the two are
//! each other's one candidate; otherwise it stays blank.

use mercury_adapter::monitor::MonitorBroadcast;
use std::collections::VecDeque;
use std::time::Duration;
use tokio::time::Instant;

/// A report and a frame further apart than this are not each other's.
pub const SNR_WINDOW: Duration = Duration::from_secs(2);
/// A decided frame, or a report, can still matter to a later frame this long.
const FORGET: Duration = Duration::from_secs(4 * SNR_WINDOW.as_secs());
/// At most this many reports, and as many frames, are held.
const KEPT: usize = 32;

#[derive(Debug)]
struct Report {
    report: MonitorBroadcast,
    at: Instant,
}

#[derive(Debug)]
struct Frame {
    /// The stored row waiting for an S/N; `None` for a frame nothing was
    /// stored for (a beacon, another program's data), which still counts.
    row: Option<i64>,
    at: Instant,
    decided: bool,
}

#[derive(Debug, Default)]
pub struct Matcher {
    reports: VecDeque<Report>,
    frames: VecDeque<Frame>,
    /// When the bound last pushed something out: around then, one candidate
    /// may only look like the only one.
    lost: Option<Instant>,
}

fn apart(a: Instant, b: Instant) -> Duration {
    a.max(b) - a.min(b)
}

impl Matcher {
    pub fn report(&mut self, report: MonitorBroadcast, now: Instant) {
        self.forget(now);
        if self.reports.len() >= KEPT {
            self.reports.pop_front();
            self.lost = Some(now);
        }
        self.reports.push_back(Report { report, at: now });
    }

    pub fn frame(&mut self, row: Option<i64>, now: Instant) {
        self.forget(now);
        if self.frames.len() >= KEPT {
            self.frames.pop_front();
            self.lost = Some(now);
        }
        self.frames.push_back(Frame { row, at: now, decided: false });
    }

    fn forget(&mut self, now: Instant) {
        let old = |at: Instant| now.saturating_duration_since(at) > FORGET;
        self.reports.retain(|r| !old(r.at));
        self.frames.retain(|f| !f.decided || !old(f.at));
    }

    /// The reports within the window of a frame that arrived at `at`.
    fn near(&self, at: Instant) -> impl Iterator<Item = &Report> {
        self.reports.iter().filter(move |r| apart(r.at, at) <= SNR_WINDOW)
    }

    /// When the frame can be decided: once its window has closed and, if it
    /// has one candidate, that report's window too (until then another frame
    /// could turn out to be the report's).
    fn due(&self, f: &Frame) -> Instant {
        let mut near = self.near(f.at);
        match (near.next(), near.next()) {
            (Some(only), None) => f.at.max(only.at) + SNR_WINDOW,
            _ => f.at + SNR_WINDOW,
        }
    }

    /// The report for `self.frames[i]`, if the two are each other's only one.
    fn pair(&self, i: usize) -> Option<MonitorBroadcast> {
        let f = &self.frames[i];
        if self.lost.is_some_and(|lost| f.at <= lost + FORGET) {
            return None;
        }
        let mut near = self.near(f.at);
        let (Some(only), None) = (near.next(), near.next()) else { return None };
        let another = self.frames.iter().enumerate().any(|(j, o)| j != i && apart(o.at, only.at) <= SNR_WINDOW);
        // 0.0 is Mercury's "unknown".
        (!another && only.report.snr_db != 0.0).then(|| only.report.clone())
    }

    /// Decide every frame that is due: the rows that got a report.
    pub fn settle(&mut self, now: Instant) -> Vec<(i64, MonitorBroadcast)> {
        let mut out = Vec::new();
        for i in 0..self.frames.len() {
            if self.frames[i].decided || now < self.due(&self.frames[i]) {
                continue;
            }
            if let (Some(row), Some(report)) = (self.frames[i].row, self.pair(i)) {
                out.push((row, report));
            }
            self.frames[i].decided = true;
        }
        self.forget(now);
        out
    }

    /// When [`Matcher::settle`] next has something to decide.
    pub fn wake(&self) -> Option<Instant> {
        self.frames.iter().filter(|f| !f.decided).map(|f| self.due(f)).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn report(snr_db: f32) -> MonitorBroadcast {
        MonitorBroadcast { mode: "DATAC3".into(), snr_db }
    }

    fn ids(out: &[(i64, MonitorBroadcast)]) -> Vec<(i64, f32)> {
        out.iter().map(|(id, r)| (*id, r.snr_db)).collect()
    }

    /// Everything the matcher still decides, however late.
    fn rest(m: &mut Matcher, t0: Instant) -> Vec<(i64, f32)> {
        ids(&m.settle(at(t0, 60_000)))
    }

    #[test]
    fn report_then_frame_matches() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.report(report(7.5), at(t0, 0));
        m.frame(Some(1), at(t0, 10));
        assert_eq!(ids(&m.settle(at(t0, 2_010))), [(1, 7.5)]);
    }

    #[test]
    fn frame_then_report_matches() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        m.report(report(7.5), at(t0, 1));
        assert_eq!(ids(&m.settle(at(t0, 2_001))), [(1, 7.5)]);
        assert_eq!(rest(&mut m, t0), [], "decided once");
    }

    #[test]
    fn nothing_before_the_window_closes() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        m.report(report(7.5), at(t0, 500));
        // The frame's window has closed, but not yet the report's: another
        // frame could still be the report's.
        assert_eq!(ids(&m.settle(at(t0, 2_100))), []);
        assert_eq!(ids(&m.settle(at(t0, 2_500))), [(1, 7.5)]);
    }

    #[test]
    fn no_report_stays_blank() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        assert_eq!(rest(&mut m, t0), []);
        assert_eq!(m.wake(), None);
    }

    #[test]
    fn late_report_is_ignored() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        m.report(report(7.5), at(t0, 2_001));
        assert_eq!(rest(&mut m, t0), []);
    }

    #[test]
    fn two_reports_one_frame_blank() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.report(report(7.5), at(t0, 0));
        m.frame(Some(1), at(t0, 100));
        m.report(report(3.0), at(t0, 200));
        assert_eq!(rest(&mut m, t0), []);
    }

    #[test]
    fn two_frames_one_report_blank_both() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        m.report(report(7.5), at(t0, 100));
        m.frame(Some(2), at(t0, 300));
        assert_eq!(rest(&mut m, t0), []);
    }

    #[test]
    fn a_second_frame_after_the_firsts_window_still_blanks_both() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        m.report(report(7.5), at(t0, 1_900));
        assert_eq!(ids(&m.settle(at(t0, 2_000))), []);
        m.frame(Some(2), at(t0, 3_500));
        assert_eq!(rest(&mut m, t0), []);
    }

    #[test]
    fn an_unstored_frame_still_makes_it_ambiguous() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(None, at(t0, 0));
        m.report(report(7.5), at(t0, 100));
        m.frame(Some(2), at(t0, 200));
        assert_eq!(rest(&mut m, t0), []);
    }

    #[test]
    fn zero_is_unknown() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        m.report(report(0.0), at(t0, 1));
        assert_eq!(rest(&mut m, t0), []);
        // ...but it is still a report: with a second one, neither is used.
        m.frame(Some(2), at(t0, 100_000));
        m.report(report(0.0), at(t0, 100_001));
        m.report(report(5.0), at(t0, 100_002));
        assert_eq!(ids(&m.settle(at(t0, 200_000))), []);
    }

    #[test]
    fn separate_receptions_each_match() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        m.frame(Some(1), at(t0, 0));
        m.report(report(7.5), at(t0, 1));
        assert_eq!(ids(&m.settle(at(t0, 2_001))), [(1, 7.5)]);
        m.report(report(-2.0), at(t0, 10_000));
        m.frame(Some(2), at(t0, 10_001));
        assert_eq!(ids(&m.settle(at(t0, 12_001))), [(2, -2.0)]);
    }

    #[test]
    fn a_flood_stays_bounded() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        for i in 0..500 {
            m.report(report(7.5), at(t0, i));
            m.frame(Some(i as i64), at(t0, i));
        }
        assert!(m.reports.len() <= KEPT && m.frames.len() <= KEPT);
        assert_eq!(rest(&mut m, t0), [], "what was forgotten must not make the rest look certain");
    }

    #[test]
    fn evicted_reports_do_not_make_one_look_certain() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        for i in 0..=KEPT as u64 {
            m.report(report(7.5), at(t0, i));
        }
        m.frame(Some(1), at(t0, 100));
        assert_eq!(rest(&mut m, t0), []);
    }

    #[test]
    fn wake_is_the_earliest_undecided_frame() {
        let (t0, mut m) = (Instant::now(), Matcher::default());
        assert_eq!(m.wake(), None);
        m.frame(Some(1), at(t0, 0));
        m.frame(Some(2), at(t0, 10_000));
        assert_eq!(m.wake(), Some(at(t0, 2_000)));
        m.settle(at(t0, 2_000));
        assert_eq!(m.wake(), Some(at(t0, 12_000)));
        m.report(report(7.5), at(t0, 11_000));
        assert_eq!(m.wake(), Some(at(t0, 13_000)), "the report's window too");
    }
}
