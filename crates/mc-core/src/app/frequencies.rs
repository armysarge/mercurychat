//! The frequency list as the UI sees it (M3b, D15): the stored rows with
//! their band, and the checks an entry must pass before it is stored.

use crate::band::band_for;
use crate::presets::Preset;
use crate::profile::FieldError;
use mc_storage::FrequencyRow;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Highest frequency an entry may hold (6 m tops out at 54 MHz).
pub const MAX_ENTRY_HZ: u64 = 60_000_000;
/// The modes allowed when the rig has not said which it has.
pub const DEFAULT_MODES: [&str; 8] = ["USB", "LSB", "PKTUSB", "PKTLSB", "CW", "AM", "FM", "RTTY"];
pub const PURPOSES: [&str; 3] = ["beacon", "cq", "chat"];
/// The setting that records the first-run suggestions were added.
pub const SEEDED_SETTING: &str = "rig.presets_seeded";

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct FrequencyEntry {
    /// Empty for a new entry.
    pub id: String,
    pub name: String,
    #[ts(type = "number")]
    pub freq_hz: u64,
    pub mode: String,
    pub passband_hz: Option<i32>,
    /// `beacon`, `cq`, `chat`.
    pub purpose: Vec<String>,
    pub notes: String,
    /// A shipped suggestion (labelled so wherever it appears).
    pub suggested: bool,
    pub preset_key: Option<String>,
    pub region: Option<u8>,
    #[ts(type = "number")]
    pub sort: i64,
    /// Derived from the frequency; `null` outside the amateur bands.
    pub band: Option<String>,
    /// Outside every amateur band, kept on purpose (a warning in the UI).
    pub out_of_band: bool,
}

impl From<FrequencyRow> for FrequencyEntry {
    fn from(r: FrequencyRow) -> Self {
        let band = band_for(r.freq_hz).map(String::from);
        FrequencyEntry {
            out_of_band: band.is_none(),
            band,
            id: r.id,
            name: r.name,
            freq_hz: r.freq_hz,
            mode: r.mode,
            passband_hz: r.passband_hz,
            purpose: r.purpose,
            notes: r.notes,
            suggested: r.suggested,
            preset_key: r.preset_key,
            region: r.region,
            sort: r.sort,
        }
    }
}

fn err(field: &str, message: &str) -> FieldError {
    FieldError { field: field.into(), message: message.into() }
}

impl FrequencyEntry {
    /// Check an entry from the UI; `modes` are the rig's own, when known.
    pub fn validate(&self, modes: Option<&[String]>) -> Result<(), Vec<FieldError>> {
        let mut errors = vec![];
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            errors.push(err("name", "Enter a name of 1 to 64 characters"));
        } else if name.chars().any(char::is_control) {
            errors.push(err("name", "The name has a control character"));
        }
        if self.freq_hz == 0 || self.freq_hz > MAX_ENTRY_HZ {
            errors.push(err("freqHz", "Enter a frequency up to 60 MHz"));
        } else if band_for(self.freq_hz).is_none() && !self.out_of_band {
            errors.push(err("freqHz", "Outside the amateur bands: mark it Out of band to keep it"));
        }
        let known = match modes {
            Some(m) if !m.is_empty() => m.contains(&self.mode),
            _ => DEFAULT_MODES.contains(&self.mode.as_str()),
        };
        if !known {
            errors.push(err("mode", "Choose a mode this rig has"));
        }
        if self.passband_hz.is_some_and(|p| !(0..=20_000).contains(&p)) {
            errors.push(err("passbandHz", "Enter a passband up to 20000 Hz, or leave it empty"));
        }
        if self.purpose.iter().any(|p| !PURPOSES.contains(&p.as_str())) {
            errors.push(err("purpose", "Purposes are beacon, cq and chat"));
        }
        if self.notes.chars().count() > 500 {
            errors.push(err("notes", "Notes are at most 500 characters"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// As stored. `id` must be set.
    pub fn to_row(&self) -> FrequencyRow {
        FrequencyRow {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            freq_hz: self.freq_hz,
            mode: self.mode.clone(),
            passband_hz: self.passband_hz,
            purpose: self.purpose.clone(),
            notes: self.notes.clone(),
            suggested: self.suggested,
            preset_key: self.preset_key.clone(),
            region: self.region,
            sort: self.sort,
        }
    }
}

/// A region's suggestions as rows.
pub fn preset_rows(region: u8, presets: &[Preset]) -> Vec<FrequencyRow> {
    presets
        .iter()
        .map(|p| FrequencyRow {
            id: format!("preset-{}", p.key),
            name: p.name.into(),
            freq_hz: p.freq_hz,
            mode: p.mode.into(),
            passband_hz: None,
            purpose: p.purpose.iter().map(|s| s.to_string()).collect(),
            notes: String::new(),
            suggested: true,
            preset_key: Some(p.key.into()),
            region: Some(region),
            sort: 0,
        })
        .collect()
}
