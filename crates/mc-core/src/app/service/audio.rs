//! The actor's side of Settings → Audio (M3b.1 design §6): Mercury's device
//! lists from its UI link, and `set_audio_config` sent only while the station
//! is idle, with Mercury's own verdict written to the activity log.

use super::{conflict, App};
use crate::app::audio::{AudioProbe, AudioWait, AUDIO_CONFIRM, CHANNELS, PREFERRED_SYSTEM};
use crate::app::contract::{AppError, CommandOk};
use crate::app::rig::{Activity, RigRefusal};
use crate::profile::ModemTarget;
use mercury_adapter::telemetry::{TelemetryCmd, TelemetryLink, TelemetryMsg};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::Instant;

fn invalid(field: &str, message: &str) -> AppError {
    AppError::Validation { errors: crate::app::chat::field(field, message) }
}

fn token_ok(s: &str) -> bool {
    s.len() <= 512 && !s.chars().any(char::is_control)
}

impl App {
    pub(super) fn audio_settings(&mut self) -> Result<CommandOk, AppError> {
        self.audio.collect_probe();
        self.start_audio_probe();
        Ok(CommandOk::Audio(self.audio.view()))
    }

    /// Owner, 24 Nov: list the sound cards before one is chosen. A built-in
    /// Mercury runs on null audio until then (M5 §5.5: no card opened), and
    /// lists nothing; `mercury -z -x <this computer's usual system>` lists the
    /// cards and exits, opening none. Off the actor; the next request shows it.
    fn start_audio_probe(&mut self) {
        if !self.audio.wants_probe() {
            return;
        }
        let builtin = matches!(self.active_profile().map(|p| p.modem), Some(ModemTarget::BuiltIn { .. }));
        let Some(bundle) = self.bundle.clone().filter(|_| builtin) else { return };
        if self.audio.probing.swap(true, Ordering::SeqCst) {
            return;
        }
        let (slot, probing) = (self.audio.incoming.clone(), self.audio.probing.clone());
        tokio::spawn(async move {
            let found = async {
                let exe = tokio::task::spawn_blocking(move || bundle.verify_program(crate::bundle::MERCURY_EXE))
                    .await
                    .ok()?
                    .ok()?;
                let mut cmd = mercury_adapter::child::command(&exe);
                cmd.args(["-z", "-x", PREFERRED_SYSTEM]).stdout(std::process::Stdio::piped());
                let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok()?;
                Some(AudioProbe::from_listing(PREFERRED_SYSTEM, &String::from_utf8_lossy(&out.stdout)))
            }
            .await;
            if let (Some(p), Ok(mut g)) = (found, slot.lock()) {
                *g = Some(p);
            }
            probing.store(false, Ordering::SeqCst);
        });
    }

    pub(super) fn set_audio(
        &mut self,
        system: String,
        input: String,
        output: String,
        channel: String,
    ) -> Result<CommandOk, AppError> {
        if !CHANNELS.contains(&channel.as_str()) {
            return Err(invalid("channel", "Choose left, right or stereo"));
        }
        if !token_ok(&system) || !token_ok(&input) || !token_ok(&output) {
            return Err(invalid("device", "Choose a device from the list"));
        }
        let refusal = match self.activity() {
            _ if self.rig.tune.active => Some(RigRefusal::Tuning),
            Activity::Idle => None,
            Activity::Calling => Some(RigRefusal::Calling),
            Activity::Connected => Some(RigRefusal::Connected),
            Activity::Draining => Some(RigRefusal::Draining),
            Activity::Broadcasting => Some(RigRefusal::Broadcasting),
            Activity::Tuning => Some(RigRefusal::Tuning),
        };
        if let Some(r) = refusal {
            return Err(AppError::Refused { reason: r.key(), message: r.message().to_string() });
        }
        let tx = match (&self.rig.tele_tx, self.audio.link_up) {
            (Some(tx), true) => tx.clone(),
            _ => {
                return Err(conflict(
                    "Mercury's UI link is off: start Mercury with -G (or ui_enabled = true in mercury.ini)".into(),
                ))
            }
        };
        let cmd = TelemetryCmd::SetAudioConfig { capture: input, playback: output, channel, system };
        tx.try_send(cmd).map_err(|_| conflict("Mercury's UI link is not accepting commands".into()))?;
        self.audio.wait = Some(AudioWait::Lists(Instant::now() + AUDIO_CONFIRM));
        Ok(CommandOk::Done)
    }

    /// What the audio side reads from Mercury's UI link.
    pub(super) fn audio_telemetry_msg(&mut self, msg: &TelemetryMsg) {
        let now = Instant::now();
        match msg {
            TelemetryMsg::Link(TelemetryLink::Up) => self.audio.link_up = true,
            TelemetryMsg::Link(_) => {
                if self.audio.wait.take().is_some() {
                    self.audio_note("Audio not applied: Mercury's UI link was lost".into());
                }
                self.audio.reset();
            }
            TelemetryMsg::Audio(ev) => self.audio.on_event(ev, now),
            TelemetryMsg::Status(st) => {
                if st.audio_ok.is_some() {
                    self.audio.view.audio_ok = st.audio_ok;
                    self.audio.view.audio_error = st.audio_error.clone().filter(|e| !e.is_empty());
                }
                if let (Some(AudioWait::Status(_)), Some(ok)) = (self.audio.wait, st.audio_ok) {
                    self.audio.wait = None;
                    if ok {
                        self.builtin_audio_chosen();
                    }
                    let text = if ok {
                        "Audio applied: Mercury saved the new devices".to_string()
                    } else {
                        let why = self.audio.view.audio_error.clone().unwrap_or_else(|| "no reason given".into());
                        format!("Audio not applied: {why}")
                    };
                    self.audio_note(text);
                }
            }
            TelemetryMsg::RadioList(_) | TelemetryMsg::CommandFailed { .. } => {}
        }
    }

    /// A `set_audio_config` Mercury has not answered in time.
    pub(super) fn sync_audio(&mut self) {
        if self.audio.wake().is_some_and(|t| Instant::now() >= t) {
            self.audio.wait = None;
            self.audio_note("Audio not confirmed: Mercury did not report the result within 5 s".into());
        }
    }

    fn audio_note(&mut self, text: String) {
        let line = self.discovery.note("audio", text);
        self.emit(line);
    }
}
