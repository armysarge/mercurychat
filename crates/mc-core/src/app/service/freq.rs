//! The actor's side of the frequency list, the region's suggestions and the
//! QSY schedule (M3b design §6.6, §6.9).

use super::{conflict, storage, App};
use crate::app::contract::{AppError, CommandOk};
use crate::app::events::AppEvent;
use crate::app::frequencies::{preset_rows, FrequencyEntry, SEEDED_SETTING};
use crate::app::rig::RigChange;
use crate::app::rigconfig::rig_key;
use crate::app::schedule::{self, Schedule, ScheduleNext};
use crate::presets::presets;
use chrono::NaiveTime;
use std::time::Duration;
use tokio::time::Instant;

/// How often the schedule is looked at.
pub const SCHEDULE_TICK: Duration = Duration::from_secs(15);
/// The region whose suggestions a profile starts with.
pub const DEFAULT_REGION: u8 = 1;

fn utc_time_now() -> NaiveTime {
    chrono::Utc::now().time()
}

pub(super) struct FreqSide {
    schedule: Schedule,
    /// The slot in force (its start) and whether it was applied (or seen).
    current: Option<String>,
    done: bool,
    next_tick: Option<Instant>,
    tick: Duration,
    clock: fn() -> NaiveTime,
    shown_next: Option<Option<ScheduleNext>>,
}

impl FreqSide {
    pub(super) fn new(clock: Option<fn() -> NaiveTime>, tick: Option<Duration>) -> FreqSide {
        FreqSide {
            schedule: Schedule::default(),
            current: None,
            done: true,
            next_tick: None,
            tick: tick.unwrap_or(SCHEDULE_TICK),
            clock: clock.unwrap_or(utc_time_now),
            shown_next: Some(None),
        }
    }

    /// When the loop must next wake for the schedule.
    pub(super) fn wake(&self) -> Option<Instant> {
        self.schedule.enabled.then_some(self.next_tick).flatten()
    }

    /// A new schedule: the slot already in force counts as applied, so
    /// enabling it (or starting the app) never retunes by itself.
    fn reset(&mut self, schedule: Schedule) {
        let now = (self.clock)();
        self.current = schedule::current(&schedule, now).map(|s| s.start_utc.clone());
        self.done = true;
        self.next_tick = Some(Instant::now());
        self.schedule = schedule;
    }
}

fn region_key(profile_id: &str) -> String {
    rig_key(profile_id, "region")
}

fn schedule_key(profile_id: &str) -> String {
    rig_key(profile_id, "schedule")
}

impl App {
    fn freq_profile(&self) -> Result<String, AppError> {
        self.active_profile().map(|p| p.id).ok_or_else(|| conflict("no station is selected".into()))
    }

    fn region(&self, profile_id: &str) -> u8 {
        self.store
            .setting(&region_key(profile_id))
            .ok()
            .flatten()
            .and_then(|s| s.parse().ok())
            .filter(|r| (1..=3).contains(r))
            .unwrap_or(DEFAULT_REGION)
    }

    pub(super) fn entries(&self) -> Result<Vec<FrequencyEntry>, AppError> {
        Ok(self.store.frequencies().map_err(storage)?.into_iter().map(FrequencyEntry::from).collect())
    }

    pub(super) fn rig_settings(&self) -> Result<CommandOk, AppError> {
        let id = self.freq_profile()?;
        let schedule = self
            .store
            .setting(&schedule_key(&id))
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        Ok(CommandOk::RigSettings {
            config: self
                .rig_config_override()
                .unwrap_or_else(|| crate::app::rigconfig::RigConfig::load(&self.store, &id)),
            region: self.region(&id),
            schedule,
        })
    }

    pub(super) fn frequencies(&self) -> Result<CommandOk, AppError> {
        Ok(CommandOk::Frequencies { entries: self.entries()? })
    }

    pub(super) fn save_frequency(&mut self, entry: FrequencyEntry) -> Result<CommandOk, AppError> {
        let modes = self.rig.policy.view(Instant::now()).caps.map(|c| c.modes);
        entry.validate(modes.as_deref()).map_err(|errors| AppError::Validation { errors })?;
        let stored = self.store.frequencies().map_err(storage)?;
        let mut entry = entry;
        match stored.iter().find(|r| r.id == entry.id && !entry.id.is_empty()) {
            // What the entry is (a suggestion, from which region) stays as stored.
            Some(old) => {
                entry.suggested = old.suggested;
                entry.preset_key = old.preset_key.clone();
                entry.region = old.region;
            }
            None => {
                entry.id = uuid::Uuid::new_v4().to_string();
                entry.suggested = false;
                entry.preset_key = None;
                entry.region = None;
            }
        }
        self.store.save_frequency(&entry.to_row()).map_err(storage)?;
        self.emit(AppEvent::FrequenciesChanged);
        self.frequencies()
    }

    pub(super) fn delete_frequency(&mut self, id: &str) -> Result<CommandOk, AppError> {
        if !self.store.delete_frequency(id).map_err(storage)? {
            return Err(conflict("that entry is not in the frequency list".into()));
        }
        self.emit(AppEvent::FrequenciesChanged);
        Ok(CommandOk::Done)
    }

    pub(super) fn restore_suggested(&mut self) -> Result<CommandOk, AppError> {
        let id = self.freq_profile()?;
        let region = self.region(&id);
        let rows = preset_rows(region, &presets(region));
        let added = self.store.insert_missing_presets(&rows).map_err(storage)?;
        if added > 0 {
            self.emit(AppEvent::FrequenciesChanged);
        }
        Ok(CommandOk::Restored { added: added as u32 })
    }

    pub(super) fn set_region(&mut self, region: u8) -> Result<CommandOk, AppError> {
        let id = self.freq_profile()?;
        if !(1..=3).contains(&region) {
            return Err(AppError::Validation {
                errors: crate::app::chat::field("region", "Choose IARU region 1, 2 or 3"),
            });
        }
        self.store.set_setting(&region_key(&id), &region.to_string()).map_err(storage)?;
        Ok(CommandOk::Done)
    }

    /// Tune to a list entry: frequency, then mode, one refusal for both.
    pub(super) fn tune_to_entry(&mut self, id: &str) -> Result<CommandOk, AppError> {
        let entry = self
            .entries()?
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| conflict("that entry is not in the frequency list".into()))?;
        self.tune_entry(&entry)
    }

    fn tune_entry(&mut self, e: &FrequencyEntry) -> Result<CommandOk, AppError> {
        let changes = [
            RigChange::Frequency { hz: e.freq_hz },
            RigChange::Mode { mode: e.mode.clone(), passband_hz: e.passband_hz.unwrap_or(0) },
        ];
        self.rig_set_all(&changes)
    }

    /// On selecting a profile: the first run's suggestions (once, ever),
    /// and the profile's schedule.
    pub(super) fn freq_profile_changed(&mut self, profile_id: Option<&str>) {
        if let Some(id) = profile_id {
            let seeded = self.store.setting(SEEDED_SETTING).ok().flatten().is_some();
            if !seeded {
                let empty = self.store.frequencies().map(|f| f.is_empty()).unwrap_or(false);
                if empty {
                    let region = self.region(id);
                    if let Err(e) = self.store.insert_missing_presets(&preset_rows(region, &presets(region))) {
                        tracing::warn!("frequency list suggestions: {e}");
                    }
                }
                let _ = self.store.set_setting(SEEDED_SETTING, "1");
            }
        }
        let schedule = profile_id
            .and_then(|id| self.store.setting(&schedule_key(id)).ok().flatten())
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        self.freq.reset(schedule);
    }

    pub(super) fn set_schedule(&mut self, schedule: Schedule) -> Result<CommandOk, AppError> {
        let id = self.freq_profile()?;
        let ids: Vec<String> = self.store.frequencies().map_err(storage)?.into_iter().map(|r| r.id).collect();
        schedule
            .validate(|e| ids.iter().any(|i| i == e))
            .map_err(|m| AppError::Validation { errors: crate::app::chat::field("schedule", m) })?;
        let json = serde_json::to_string(&schedule).map_err(|e| AppError::Internal { message: e.to_string() })?;
        self.store.set_setting(&schedule_key(&id), &json).map_err(storage)?;
        self.freq.reset(schedule);
        self.emit_schedule_next();
        Ok(CommandOk::Done)
    }

    /// The next scheduled change, if the schedule is on.
    pub(super) fn schedule_next(&self) -> Option<ScheduleNext> {
        let (at, slot) = schedule::next(&self.freq.schedule, (self.freq.clock)())?;
        let name = self.store.frequencies().ok()?.into_iter().find(|r| r.id == slot.entry_id)?.name;
        Some(ScheduleNext { at_utc: at, entry_name: name })
    }

    fn emit_schedule_next(&mut self) {
        let next = self.schedule_next();
        if self.freq.shown_next.as_ref() != Some(&next) {
            self.freq.shown_next = Some(next.clone());
            self.emit(AppEvent::ScheduleNext { next });
        }
    }

    fn entry_name(&self, id: &str) -> Option<FrequencyEntry> {
        self.entries().ok()?.into_iter().find(|e| e.id == id)
    }

    /// Each tick: apply the slot in force once the gate allows it; a slot
    /// still waiting when the next starts is skipped, and the log says so.
    pub(super) fn sync_schedule(&mut self, now: Instant) {
        if !self.freq.schedule.enabled || self.freq.next_tick.is_some_and(|t| now < t) {
            return;
        }
        self.freq.next_tick = Some(now + self.freq.tick);
        let time = (self.freq.clock)();
        let slot = schedule::current(&self.freq.schedule, time).cloned();
        let start = slot.as_ref().map(|s| s.start_utc.clone());
        if start != self.freq.current {
            if let (Some(old), false) = (self.freq.current.clone(), self.freq.done) {
                let name = self
                    .freq
                    .schedule
                    .slots
                    .iter()
                    .find(|s| s.start_utc == old)
                    .and_then(|s| self.entry_name(&s.entry_id))
                    .map_or_else(|| "an entry".to_string(), |e| e.name);
                let line = self
                    .discovery
                    .note("rig", format!("Scheduled QSY to {name} at {old} UTC skipped: the station was busy"));
                self.emit(line);
            }
            self.freq.current = start;
            self.freq.done = false;
        }
        if let (Some(slot), false) = (slot, self.freq.done) {
            match self.entry_name(&slot.entry_id) {
                None => {
                    self.freq.done = true;
                    let text = format!("Scheduled QSY at {} UTC skipped: its entry was deleted", slot.start_utc);
                    let line = self.discovery.note("rig", text);
                    self.emit(line);
                }
                Some(e) => {
                    // Refused (busy, PTT, link down, a QSY under way): try again next tick.
                    if !self.qsy_pending() && self.tune_entry(&e).is_ok() {
                        self.freq.done = true;
                        let line = self.discovery.note("rig", format!("Scheduled QSY to {}", e.name));
                        self.emit(line);
                    }
                }
            }
        }
        self.emit_schedule_next();
    }
}
