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

/// This computer's usual sound system: chosen for the operator (owner, 24 Nov).
pub const PREFERRED_SYSTEM: &str = if cfg!(windows) { "wasapi" } else { "pulse" };

/// `mercury -z -x <system>`'s listing: (playback, capture) devices. A listing
/// that failed ("Error in audio->init()...") gives two empty lists.
#[cfg(test)]
pub fn parse_device_list(text: &str) -> (Vec<AudioDeviceView>, Vec<AudioDeviceView>) {
    let (out, inp, _, _) = parse_listing(text);
    (out, inp)
}

const QUOTE: char = '\'';

/// The lists and each list's system default.
fn parse_listing(text: &str) -> (Vec<AudioDeviceView>, Vec<AudioDeviceView>, Option<String>, Option<String>) {
    let (mut out, mut inp) = (Vec::new(), Vec::new());
    let (mut def_out, mut def_in) = (None, None);
    let mut capture = None;
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with("playback devices:") {
            capture = Some(false);
        } else if line.starts_with("capture devices:") {
            capture = Some(true);
        } else if let (Some(cap), Some(rest)) = (capture, line.strip_prefix("device: name: '")) {
            // device: name: '<name>'  id: '<id>'[  (system default)]
            let Some((name, rest)) = rest.split_once("'  id: '") else { continue };
            let Some((id, tail)) = rest.rsplit_once(QUOTE) else { continue };
            let d = AudioDeviceView { id: id.to_string(), name: name.to_string() };
            let default = tail.contains("(system default)");
            if cap {
                if default && def_in.is_none() {
                    def_in = Some(d.id.clone());
                }
                inp.push(d);
            } else {
                if default && def_out.is_none() {
                    def_out = Some(d.id.clone());
                }
                out.push(d);
            }
        }
    }
    (out, inp, def_out, def_in)
}

/// The sound cards of `system`, listed by `mercury -z` while Mercury itself
/// runs on null audio (M5 §5.5: no card is opened until one is chosen).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AudioProbe {
    pub(crate) system: String,
    pub(crate) inputs: Vec<AudioDeviceView>,
    pub(crate) outputs: Vec<AudioDeviceView>,
    pub(crate) default_input: Option<String>,
    pub(crate) default_output: Option<String>,
}

impl AudioProbe {
    pub(crate) fn from_listing(system: &str, text: &str) -> AudioProbe {
        let (outputs, inputs, default_output, default_input) = parse_listing(text);
        AudioProbe { system: system.to_string(), inputs, outputs, default_input, default_output }
    }
}

/// What the actor knows of Mercury's audio.
#[derive(Debug, Default)]
pub(crate) struct AudioSide {
    pub(crate) link_up: bool,
    pub(crate) view: AudioSettingsView,
    pub(crate) wait: Option<AudioWait>,
    /// The cards listed by `mercury -z`, for while Mercury is on null audio.
    pub(crate) probe: Option<AudioProbe>,
    /// Where the probe's task leaves its result, and whether one is running.
    pub(crate) incoming: std::sync::Arc<std::sync::Mutex<Option<AudioProbe>>>,
    pub(crate) probing: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn devices(list: &[AudioDevice]) -> Vec<AudioDeviceView> {
    list.iter().map(|d| AudioDeviceView { id: d.id.clone(), name: d.name.clone() }).collect()
}

impl AudioSide {
    pub(crate) fn view(&self) -> AudioSettingsView {
        let mut v = AudioSettingsView { available: self.link_up, ..self.view.clone() };
        // On null audio Mercury lists nothing: offer the probed cards, on this
        // computer's usual system, the system defaults preselected.
        if v.system == "null" {
            if let Some(p) = &self.probe {
                v.system = p.system.clone();
                if !v.systems.contains(&p.system) {
                    v.systems.push(p.system.clone());
                }
                v.inputs = p.inputs.clone();
                v.outputs = p.outputs.clone();
                v.input =
                    p.default_input.clone().or_else(|| p.inputs.first().map(|d| d.id.clone())).unwrap_or_default();
                v.output =
                    p.default_output.clone().or_else(|| p.outputs.first().map(|d| d.id.clone())).unwrap_or_default();
            }
        }
        v
    }

    /// Mercury is on null audio and no cards have been listed for it yet.
    pub(crate) fn wants_probe(&self) -> bool {
        self.link_up && self.view.system == "null" && self.probe.is_none()
    }

    /// Take a finished probe's result, if one has arrived.
    pub(crate) fn collect_probe(&mut self) {
        if let Some(p) = self.incoming.lock().ok().and_then(|mut g| g.take()) {
            self.probe = Some(p);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "playback devices:\n\
device: name: 'Speakers (Realtek(R) Audio)'  id: '{0.0.0.00000000}.{aaa}'  (system default)\n\
device: name: 'USB Audio CODEC'  id: '{0.0.0.00000000}.{bbb}'\n\
capture devices:\n\
device: name: 'Microphone Array'  id: '{0.0.1.00000000}.{ccc}'  (system default)\n\
device: name: 'USB Audio CODEC'  id: '{0.0.1.00000000}.{ddd}'\n";

    /// Owner, 24 Nov: the sound cards are listed before one is chosen, from
    /// `mercury -z -x <system>` (Mercury itself stays on null audio).
    #[test]
    fn the_listing_gives_both_lists() {
        let (outputs, inputs) = parse_device_list(LISTING);
        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[0],
            AudioDeviceView { id: "{0.0.0.00000000}.{aaa}".into(), name: "Speakers (Realtek(R) Audio)".into() }
        );
        assert_eq!(inputs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Microphone Array", "USB Audio CODEC"]);
        assert_eq!(inputs[1].id, "{0.0.1.00000000}.{ddd}");
    }

    #[test]
    fn a_failed_listing_gives_nothing() {
        let (o, i) = parse_device_list("Error in audio->init(): no server\n");
        assert!(o.is_empty() && i.is_empty());
    }

    /// While Mercury runs on null its own lists are empty: the probe's are
    /// shown, with this computer's usual system chosen.
    #[test]
    fn null_audio_shows_the_probed_cards() {
        let mut side = AudioSide { link_up: true, ..Default::default() };
        side.view.system = "null".into();
        side.view.systems = vec!["null".into(), PREFERRED_SYSTEM.into()];
        side.probe = Some(AudioProbe::from_listing(PREFERRED_SYSTEM, LISTING));
        let v = side.view();
        assert_eq!(v.system, PREFERRED_SYSTEM);
        assert_eq!(v.inputs.len(), 2);
        assert_eq!(v.outputs.len(), 2);
        assert_eq!(v.input, "{0.0.1.00000000}.{ccc}", "the system default is preselected");
        // Once Mercury runs on a real system, its own report wins.
        side.view.system = PREFERRED_SYSTEM.into();
        side.view.inputs = vec![AudioDeviceView { id: "x".into(), name: "X".into() }];
        assert_eq!(side.view().inputs.len(), 1);
    }
}
