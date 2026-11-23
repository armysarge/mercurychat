//! The first-run wizard's progress (M5 §6): kept in the app settings, so a
//! restart resumes it where it stopped. While it has not reached "How you
//! operate", the station it is setting up is held in Receive only: a
//! half-made station never answers a call (Review Focus 4).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The setting holding the wizard's progress, as JSON.
pub const WIZARD_SETTING: &str = "wizard.state";

/// The wizard's steps, in order (05 §M5 steps 1–6 and 8, and the tour).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WizardStep {
    #[default]
    Welcome,
    Station,
    Modem,
    Audio,
    Rig,
    Operate,
    Tour,
    Done,
}

/// What the Station step asked, kept until the Modem step makes the profile.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct WizardStation {
    pub callsign: String,
    pub name: Option<String>,
    pub grid: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct WizardState {
    pub step: WizardStep,
    /// The profile the Modem step made; held in Receive only until the
    /// operate step is answered.
    pub draft_profile_id: Option<String>,
    pub done: bool,
    /// The steps the operator skipped, in step order, once each; Settings
    /// lists them.
    pub skipped: Vec<WizardStep>,
    pub station: Option<WizardStation>,
}

impl WizardState {
    /// The draft is held in Receive only: the operate step is not answered.
    pub fn holds_receive_only(&self) -> bool {
        !self.done && self.step <= WizardStep::Operate
    }

    /// Skipped steps in order, once each.
    pub fn normalised(mut self) -> Self {
        self.skipped.sort();
        self.skipped.dedup();
        self
    }

    /// The stored progress, or for an install without it: done when it
    /// already has stations (they were set up before the wizard existed),
    /// else the start.
    pub fn from_setting(stored: Option<&str>, has_profiles: bool) -> WizardState {
        match stored.and_then(|s| serde_json::from_str::<WizardState>(s).ok()) {
            Some(w) => w.normalised(),
            None if has_profiles => WizardState { step: WizardStep::Done, done: true, ..WizardState::default() },
            None => WizardState::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hold_ends_after_the_operate_step() {
        let at = |step| WizardState { step, ..WizardState::default() };
        assert!(at(WizardStep::Rig).holds_receive_only());
        assert!(at(WizardStep::Operate).holds_receive_only());
        assert!(!at(WizardStep::Tour).holds_receive_only());
        assert!(!WizardState { done: true, ..at(WizardStep::Welcome) }.holds_receive_only());
    }

    #[test]
    fn a_bad_setting_reads_as_the_start() {
        assert_eq!(WizardState::from_setting(Some("not json"), false), WizardState::default());
        assert!(WizardState::from_setting(None, true).done);
    }
}
