//! The frequency schedule (M3b design §6.9): time-of-day QSY between
//! frequency-list entries, per station profile, off by default. The actor
//! applies a slot when it starts, only if the rig gate allows it; a slot the
//! station is too busy for waits, and is skipped once the next one starts.

use chrono::NaiveTime;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// At most this many slots in a day.
pub const MAX_SLOTS: usize = 48;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Schedule {
    pub enabled: bool,
    pub slots: Vec<ScheduleSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScheduleSlot {
    /// `HH:MM`, UTC.
    pub start_utc: String,
    /// The frequency-list entry to tune to.
    pub entry_id: String,
}

/// The next scheduled change, for the station strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScheduleNext {
    /// `HH:MM`, UTC.
    pub at_utc: String,
    pub entry_name: String,
}

/// `HH:MM` as a time of day.
pub fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    NaiveTime::from_hms_opt(h.parse().ok()?, m.parse().ok()?, 0)
}

/// The slots with valid times, by start.
fn sorted(schedule: &Schedule) -> Vec<(NaiveTime, &ScheduleSlot)> {
    let mut v: Vec<_> = schedule.slots.iter().filter_map(|s| parse_hhmm(&s.start_utc).map(|t| (t, s))).collect();
    v.sort_by_key(|(t, _)| *t);
    v
}

/// The slot in force at `now`: the latest one started today, or else the
/// last one of the day (it started yesterday). `None` if disabled or empty.
pub fn current(schedule: &Schedule, now_utc: NaiveTime) -> Option<&ScheduleSlot> {
    if !schedule.enabled {
        return None;
    }
    let v = sorted(schedule);
    v.iter().rev().find(|(t, _)| *t <= now_utc).or(v.last()).map(|(_, s)| *s)
}

/// The slot to apply now: the one in force, unless it is `last_applied`
/// (its `start_utc`).
pub fn due<'a>(schedule: &'a Schedule, now_utc: NaiveTime, last_applied: Option<&str>) -> Option<&'a ScheduleSlot> {
    current(schedule, now_utc).filter(|s| Some(s.start_utc.as_str()) != last_applied)
}

/// The next slot to start after `now` (wrapping to tomorrow's first), with
/// its start time.
pub fn next(schedule: &Schedule, now_utc: NaiveTime) -> Option<(String, &ScheduleSlot)> {
    if !schedule.enabled {
        return None;
    }
    let v = sorted(schedule);
    v.iter().find(|(t, _)| *t > now_utc).or(v.first()).map(|(_, s)| (s.start_utc.clone(), *s))
}

impl Schedule {
    /// Times are `HH:MM` and distinct, every entry exists, at most 48 slots.
    pub fn validate(&self, entry_exists: impl Fn(&str) -> bool) -> Result<(), String> {
        if self.slots.len() > MAX_SLOTS {
            return Err(format!("At most {MAX_SLOTS} changes a day"));
        }
        let mut seen = std::collections::HashSet::new();
        for s in &self.slots {
            let Some(t) = parse_hhmm(&s.start_utc) else {
                return Err(format!("{:?} is not a time: use HH:MM, UTC", s.start_utc));
            };
            if !seen.insert(t) {
                return Err(format!("Two changes at {}", s.start_utc));
            }
            if !entry_exists(&s.entry_id) {
                return Err(format!("The change at {} names an entry that is not in the list", s.start_utc));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> NaiveTime {
        parse_hhmm(s).unwrap()
    }

    fn sched(enabled: bool, starts: &[&str]) -> Schedule {
        let slots =
            starts.iter().map(|s| ScheduleSlot { start_utc: s.to_string(), entry_id: format!("e{s}") }).collect();
        Schedule { enabled, slots }
    }

    #[test]
    fn due_picks_the_latest_started_slot() {
        let s = sched(true, &["18:00", "08:00", "12:00"]);
        assert_eq!(due(&s, t("13:00"), None).unwrap().start_utc, "12:00");
        assert_eq!(due(&s, t("12:00"), None).unwrap().start_utc, "12:00", "a slot starts on its minute");
        assert_eq!(due(&s, t("11:59"), None).unwrap().start_utc, "08:00");
        assert_eq!(due(&s, t("13:00"), Some("12:00")), None, "already applied");
        assert_eq!(due(&s, t("18:30"), Some("12:00")).unwrap().start_utc, "18:00");
        assert_eq!(next(&s, t("13:00")).unwrap().0, "18:00");
    }

    #[test]
    fn due_wraps_midnight() {
        let s = sched(true, &["06:00", "22:00"]);
        assert_eq!(due(&s, t("01:00"), None).unwrap().start_utc, "22:00", "started yesterday");
        assert_eq!(due(&s, t("23:00"), None).unwrap().start_utc, "22:00");
        assert_eq!(next(&s, t("23:00")).unwrap().0, "06:00", "tomorrow's first");
        assert_eq!(next(&s, t("05:59")).unwrap().0, "06:00");
    }

    #[test]
    fn disabled_schedule_is_never_due() {
        let s = sched(false, &["06:00", "22:00"]);
        assert_eq!(due(&s, t("07:00"), None), None);
        assert_eq!(next(&s, t("07:00")), None);
        assert_eq!(due(&sched(true, &[]), t("07:00"), None), None);
    }

    #[test]
    fn validation() {
        let all = |_: &str| true;
        assert!(sched(true, &["06:00", "22:00"]).validate(all).is_ok());
        assert!(sched(true, &["6:00"]).validate(all).is_err());
        assert!(sched(true, &["24:00"]).validate(all).is_err());
        assert!(sched(true, &["06:00", "06:00"]).validate(all).is_err());
        assert!(sched(true, &["06:00"]).validate(|_| false).is_err());
    }
}
