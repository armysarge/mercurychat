//! Mercury's sound devices, chosen in MercuryChat (M3b.1 design §6). Mercury
//! owns the choice: it lists its devices on its UI link, applies
//! `set_audio_config`, and saves it to `mercury.ini` only if its audio comes
//! back healthy. MercuryChat keeps no copy of its own.

use mercury_adapter::telemetry::{AudioDevice, AudioEvent};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::Instant;
use ts_rs::TS;

/// How long Mercury has to report the result of `set_audio_config`.
pub const AUDIO_CONFIRM: Duration = Duration::from_secs(5);
/// The input channels Mercury accepts.
pub const CHANNELS: [&str; 3] = ["left", "right", "stereo"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioDeviceView {
    pub id: String,
    pub name: String,
}

/// Settings → Audio, as Mercury reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioSettingsView {
    /// Mercury's UI link is up (it needs Mercury's `-G`).
    pub available: bool,
    /// The sound systems this Mercury build has (one: no choice to offer).
    pub systems: Vec<String>,
    pub system: String,
    pub inputs: Vec<AudioDeviceView>,
    pub outputs: Vec<AudioDeviceView>,
    /// The device ids in use.
    pub input: String,
    pub output: String,
    /// `left`, `right` or `stereo`.
    pub channel: String,
    /// Mercury's own report of its audio, from its last status.
    pub audio_ok: Option<bool>,
    pub audio_error: Option<String>,
}

/// Where a `set_audio_config` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioWait {
    /// Sent; Mercury republishes its device lists once it has applied it.
    Lists(Instant),
    /// The lists came back; the next status says whether its audio is healthy.
    Status(Instant),
}

/// What the actor knows of Mercury's audio.
#[derive(Debug, Default)]
pub(crate) struct AudioSide {
    pub(crate) link_up: bool,
    pub(crate) view: AudioSettingsView,
    pub(crate) wait: Option<AudioWait>,
}

fn devices(list: &[AudioDevice]) -> Vec<AudioDeviceView> {
    list.iter().map(|d| AudioDeviceView { id: d.id.clone(), name: d.name.clone() }).collect()
}

impl AudioSide {
    pub(crate) fn view(&self) -> AudioSettingsView {
        AudioSettingsView { available: self.link_up, ..self.view.clone() }
    }

    /// The link went down (or the modem changed): nothing Mercury said holds.
    pub(crate) fn reset(&mut self) {
        *self = AudioSide::default();
    }

    pub(crate) fn on_event(&mut self, ev: &AudioEvent, now: Instant) {
        match ev {
            AudioEvent::Capture { selected, list } => {
                self.view.inputs = devices(list);
                self.view.input = selected.clone();
                if let Some(AudioWait::Lists(_)) = self.wait {
                    self.wait = Some(AudioWait::Status(now + AUDIO_CONFIRM));
                }
            }
            AudioEvent::Playback { selected, list } => {
                self.view.outputs = devices(list);
                self.view.output = selected.clone();
            }
            AudioEvent::System { selected, list } => {
                self.view.systems = list.clone();
                self.view.system = selected.clone();
            }
            AudioEvent::Channel { selected } => self.view.channel = selected.clone(),
        }
    }

    /// When the actor must next look at the wait.
    pub(crate) fn wake(&self) -> Option<Instant> {
        match self.wait? {
            AudioWait::Lists(t) | AudioWait::Status(t) => Some(t),
        }
    }
}
