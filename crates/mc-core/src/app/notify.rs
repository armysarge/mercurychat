//! Desktop notification preferences (M3a Task 12b): app settings, not per
//! profile, kept as one JSON setting. The window decides what notifies
//! (`apps/desktop/src/notify.ts`); the core only checks and keeps the choices.

use crate::profile::FieldError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The setting the preferences are kept in.
pub const NOTIFY_SETTING: &str = "ui.notify";

/// Which events notify, and how. Every kind has its own switch under the
/// master switch; a Virtual Audio Lab profile notifies only with `lab_enabled`
/// (02 §9: the lab profile disables alerts by default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct NotifyPrefs {
    pub enabled: bool,
    /// A new incoming message, unless its conversation is open in a focused window.
    pub message: bool,
    /// An incoming call or session.
    pub incoming: bool,
    pub call_failed: bool,
    /// A message of ours going uncertain or failed.
    pub delivery: bool,
    pub heard_favourite: bool,
    /// Any newly heard station (off by default).
    pub heard_any: bool,
    /// The modem lost, faulted or recovering: urgent, shown in quiet hours.
    pub fault: bool,
    pub beacon_refused: bool,
    /// The operating system's default notification sound (never the radio's audio devices).
    pub sound: bool,
    /// Leave the message text out of message notifications.
    pub hide_text: bool,
    /// Quiet hours, local time `HH:MM`, both or neither.
    pub quiet_start: Option<String>,
    pub quiet_end: Option<String>,
    /// Notify for a Virtual Audio Lab profile too.
    pub lab_enabled: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        NotifyPrefs {
            enabled: true,
            message: true,
            incoming: true,
            call_failed: true,
            delivery: true,
            heard_favourite: true,
            heard_any: false,
            fault: true,
            beacon_refused: false,
            sound: true,
            hide_text: false,
            quiet_start: None,
            quiet_end: None,
            lab_enabled: false,
        }
    }
}

/// `HH:MM`, 00:00 to 23:59, two digits each.
fn is_hh_mm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && [0, 1, 3, 4].iter().all(|&i| b[i].is_ascii_digit())
        && s[..2].parse::<u8>().is_ok_and(|h| h < 24)
        && s[3..].parse::<u8>().is_ok_and(|m| m < 60)
}

impl NotifyPrefs {
    /// The preferences as given, or why they are refused.
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();
        for (field, v) in [("quietStart", &self.quiet_start), ("quietEnd", &self.quiet_end)] {
            if v.as_deref().is_some_and(|s| !is_hh_mm(s)) {
                errors.push(FieldError { field: field.into(), message: "Use HH:MM, from 00:00 to 23:59".into() });
            }
        }
        if self.quiet_start.is_some() != self.quiet_end.is_some() {
            errors.push(FieldError { field: "quietEnd".into(), message: "Give quiet hours a start and an end".into() });
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// The stored preferences; a missing or unreadable value reads as the defaults.
    pub fn from_setting(stored: Option<&str>) -> NotifyPrefs {
        stored
            .and_then(|s| serde_json::from_str::<NotifyPrefs>(s).ok())
            .filter(|p| p.validate().is_ok())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreadable_setting_reads_as_the_defaults() {
        for bad in [None, Some(""), Some("not json"), Some(r#"{"quietStart":"99:99","quietEnd":"01:00"}"#)] {
            assert_eq!(NotifyPrefs::from_setting(bad), NotifyPrefs::default(), "{bad:?}");
        }
        let partial = NotifyPrefs::from_setting(Some(r#"{"heardAny":true}"#));
        assert!(partial.heard_any && partial.message, "missing fields take their defaults");
    }
}
