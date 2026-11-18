//! Rig settings per station profile (M3b design §7.4): the arrangement, the
//! rigctld address and restore-on-startup, and the per-band TX and tune
//! levels. Kept as JSON in the app's settings, under the profile's keys.

use mc_storage::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// Who MercuryChat reads the rig through (RIG-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RigArrangement {
    /// No rig information at all.
    None,
    /// A rigctld the operator runs, shared with Mercury (connect-only).
    Rigctld,
    /// Mercury's own read-only frequency, from its telemetry (M3a).
    #[default]
    Mercury,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RigConfig {
    pub arrangement: RigArrangement,
    pub host: String,
    pub port: u16,
    /// Retune to the last frequency when the rig link first comes up (RIG-03: opt-in).
    pub restore_on_startup: bool,
}

impl Default for RigConfig {
    fn default() -> Self {
        RigConfig {
            arrangement: RigArrangement::Mercury,
            host: "127.0.0.1".into(),
            port: 4532,
            restore_on_startup: false,
        }
    }
}

pub(crate) fn rig_key(profile_id: &str, name: &str) -> String {
    format!("profile.{profile_id}.rig.{name}")
}

impl RigConfig {
    /// The profile's stored settings, or the defaults.
    pub fn load(store: &Store, profile_id: &str) -> RigConfig {
        store
            .setting(&rig_key(profile_id, "config"))
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, store: &mut Store, profile_id: &str) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        store.set_setting(&rig_key(profile_id, "config"), &json).map_err(|e| e.to_string())
    }
}

/// One band's transmit levels: Mercury's audio drive (`set_tx_gain`, dB)
/// and the Tune carrier's level (dBFS at the sound card, which bypasses the
/// drive on purpose: Mercury docs/TNC.md).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct BandLevels {
    pub tx_gain_db: f32,
    pub tune_dbfs: f32,
}

// Validated finite before use (the actor rejects NaN), so equality is total.
impl Eq for BandLevels {}

impl Default for BandLevels {
    fn default() -> Self {
        BandLevels { tx_gain_db: 0.0, tune_dbfs: -20.0 }
    }
}

pub const TX_GAIN_RANGE: (f32, f32) = (-20.0, 20.0);
pub const TUNE_RANGE: (f32, f32) = (-60.0, 0.0);

impl BandLevels {
    pub fn validate(&self) -> Result<(), String> {
        let within = |v: f32, (lo, hi): (f32, f32)| v.is_finite() && (lo..=hi).contains(&v);
        if !within(self.tx_gain_db, TX_GAIN_RANGE) {
            return Err(format!("TX level must be {} to {} dB", TX_GAIN_RANGE.0, TX_GAIN_RANGE.1));
        }
        if !within(self.tune_dbfs, TUNE_RANGE) {
            return Err(format!("Tune level must be {} to {} dBFS", TUNE_RANGE.0, TUNE_RANGE.1));
        }
        Ok(())
    }

    /// Every band the profile has levels for.
    pub fn load_all(store: &Store, profile_id: &str) -> BTreeMap<String, BandLevels> {
        store
            .setting(&rig_key(profile_id, "levels"))
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default()
    }

    /// The band's levels, or the defaults.
    pub fn for_band(store: &Store, profile_id: &str, band: &str) -> BandLevels {
        Self::load_all(store, profile_id).get(band).copied().unwrap_or_default()
    }

    pub fn save(store: &mut Store, profile_id: &str, band: &str, levels: BandLevels) -> Result<(), String> {
        levels.validate()?;
        let mut all = Self::load_all(store, profile_id);
        all.insert(band.to_string(), levels);
        let json = serde_json::to_string(&all).map_err(|e| e.to_string())?;
        store.set_setting(&rig_key(profile_id, "levels"), &json).map_err(|e| e.to_string())
    }
}
