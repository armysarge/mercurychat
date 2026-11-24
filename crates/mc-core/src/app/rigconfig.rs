//! Rig settings per station profile (M3b design §7.4, M3b.1 design §5): how
//! MercuryChat reads the rig (Frequency control), how Mercury keys it (PTT),
//! the CAT link for MercuryChat's own rigctld, and the per-band TX and tune
//! levels. Kept as JSON in the app's settings, under the profile's keys.
//! M3b's `arrangement` form is read and converted (M3b.1 design §8).

use crate::profile::FieldError;
use mc_storage::Store;
use mercury_adapter::rigctl::RigctldSpec;
use mercury_adapter::telemetry::{RadioList, TelemetryCmd};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use ts_rs::TS;

/// Who MercuryChat reads and sets the rig through (M3b.1 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FreqControl {
    /// No rig link: the frequency comes from Mercury's telemetry.
    #[default]
    None,
    /// A rigctld MercuryChat starts (or reuses, if one answers) on 127.0.0.1:4532.
    ManagedRigctld,
    /// A rigctld someone else runs: connect only.
    ExternalRigctld,
    /// FLRig, over XML-RPC: frequency and mode only.
    Flrig,
    /// MercuryChat's own CAT engine: the radio's commands from a CAT commands
    /// file, served to Mercury and read by the app as a rigctld (M5 Task 11e).
    CatFile,
}

/// How Mercury keys the radio, as MercuryChat sets it (M3b.1 §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PttMethod {
    /// CAT through the rigctld.
    Cat,
    /// The DTR or RTS line of a serial port.
    SerialLine,
    Cm108,
    /// No PTT: the radio's VOX.
    NoPtt,
    /// MercuryChat leaves Mercury's PTT setting as it is.
    #[default]
    LeaveMercury,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CatLink {
    #[default]
    Serial,
    /// A network radio: rigctld's `-r host:port`.
    Tcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Parity {
    #[default]
    None,
    Even,
    Odd,
}

/// A serial control line's state while the port is open (Hamlib `dtr_state`, `rts_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum LineState {
    #[default]
    Unset,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PttLine {
    #[default]
    Rts,
    Dtr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl Default for Endpoint {
    fn default() -> Self {
        Endpoint { host: "127.0.0.1".into(), port: 4532 }
    }
}

/// The CAT link of MercuryChat's own rigctld (M3b.1 §5.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct CatSettings {
    /// Hamlib rig model number.
    pub model: Option<u32>,
    /// The radio's name in the CAT commands file (built-in engine).
    pub radio: String,
    pub link: CatLink,
    pub serial_port: String,
    /// `host:port` of a network radio.
    pub tcp_addr: String,
    pub baud: u32,
    pub parity: Parity,
    pub data_bits: u8,
    pub stop_bits: u8,
    /// Wait after each write (Hamlib `post_write_delay`), ms.
    pub wait_ms: u32,
    pub dtr: LineState,
    pub rts: LineState,
}

impl Default for CatSettings {
    fn default() -> Self {
        CatSettings {
            model: None,
            radio: String::new(),
            link: CatLink::Serial,
            serial_port: String::new(),
            tcp_addr: String::new(),
            baud: 9600,
            parity: Parity::None,
            data_bits: 8,
            stop_bits: 1,
            wait_ms: 0,
            dtr: LineState::Unset,
            rts: LineState::Unset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PttSettings {
    pub method: PttMethod,
    pub line: PttLine,
    /// Line inverted (only when Mercury drives the port itself).
    pub invert: bool,
    /// A separate PTT port; empty: the CAT port.
    pub ptt_port: String,
    /// CM108 GPIO pin, 1 to 4.
    pub cm108_gpio: u8,
}

impl Default for PttSettings {
    fn default() -> Self {
        PttSettings {
            method: PttMethod::LeaveMercury,
            line: PttLine::Rts,
            invert: false,
            ptt_port: String::new(),
            cm108_gpio: 3,
        }
    }
}

/// One rig model Hamlib supports, for the model picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HamlibModelView {
    pub id: u32,
    pub mfg: String,
    pub model: String,
    pub status: String,
}

pub const READ_EVERY_CHOICES: [u8; 4] = [1, 2, 5, 10];
pub const OFFSET_LIMIT_HZ: u32 = 10_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct RigConfig {
    pub freq_control: FreqControl,
    pub external: Endpoint,
    pub flrig: Endpoint,
    pub cat: CatSettings,
    pub ptt: PttSettings,
    /// Retune to the last frequency when the rig link first comes up (RIG-03: opt-in).
    pub restore_on_startup: bool,
    /// Added to the dial frequency the rig reports; taken off when setting it.
    pub offset_hz: i32,
    /// How often the frequency and mode are read, seconds: 1, 2, 5 or 10.
    pub read_every_s: u8,
    /// The rig mode for data (QSY and the frequency test).
    pub data_mode: String,
    /// A folder to look in for rigctld when it is not on the PATH.
    pub hamlib_dir: String,
}

impl Default for RigConfig {
    fn default() -> Self {
        RigConfig {
            freq_control: FreqControl::None,
            external: Endpoint::default(),
            flrig: Endpoint { host: "127.0.0.1".into(), port: 12345 },
            cat: CatSettings::default(),
            ptt: PttSettings::default(),
            restore_on_startup: false,
            offset_hz: 0,
            read_every_s: 2,
            data_mode: "PKTUSB".into(),
            hamlib_dir: String::new(),
        }
    }
}

pub(crate) fn rig_key(profile_id: &str, name: &str) -> String {
    format!("profile.{profile_id}.rig.{name}")
}

fn field(field: &str, message: &str) -> FieldError {
    FieldError { field: field.into(), message: message.into() }
}

/// MercuryChat's own rigctld for these settings, listening at `listen`
/// (M3b.1 §5.2, §5.3). `None` without a rig model.
pub fn rigctld_spec_for(cfg: &RigConfig, listen: SocketAddr) -> Option<RigctldSpec> {
    let cat = &cfg.cat;
    let model = cat.model?;
    let (rig_path, baud, conf) = match cat.link {
        CatLink::Tcp => (cat.tcp_addr.trim().to_string(), None, vec![]),
        CatLink::Serial => {
            let parity = match cat.parity {
                Parity::None => "None",
                Parity::Even => "Even",
                Parity::Odd => "Odd",
            };
            let mut conf = vec![
                ("serial_parity".to_string(), parity.to_string()),
                ("data_bits".to_string(), cat.data_bits.to_string()),
                ("stop_bits".to_string(), cat.stop_bits.to_string()),
                ("post_write_delay".to_string(), cat.wait_ms.to_string()),
            ];
            for (name, state) in [("dtr_state", cat.dtr), ("rts_state", cat.rts)] {
                match state {
                    LineState::Unset => {}
                    LineState::On => conf.push((name.to_string(), "ON".to_string())),
                    LineState::Off => conf.push((name.to_string(), "OFF".to_string())),
                }
            }
            (cat.serial_port.trim().to_string(), Some(cat.baud), conf)
        }
    };
    let (ptt_type, ptt_path) = match cfg.ptt.method {
        PttMethod::SerialLine => {
            let line = match cfg.ptt.line {
                PttLine::Rts => "RTS",
                PttLine::Dtr => "DTR",
            };
            let port = cfg.ptt.ptt_port.trim();
            (line, (!port.is_empty()).then(|| port.to_string()))
        }
        PttMethod::NoPtt => ("NONE", None),
        // Mercury keys however it is set up; through this rigctld, that is CAT.
        // CM108 is Mercury's own (a HID device, §5.2): rigctld only reads PTT.
        PttMethod::Cat | PttMethod::LeaveMercury | PttMethod::Cm108 => ("RIG", None),
    };
    Some(RigctldSpec { model, rig_path, baud, conf, ptt_type, ptt_path, listen })
}

/// Mercury's PTT setting for these settings (M3b.1 §5.2): Mercury keys the
/// radio in every case. `None`: leave Mercury as it is.
pub fn mercury_ptt_for(cfg: &RigConfig, managed: SocketAddr) -> Option<TelemetryCmd> {
    let p = &cfg.ptt;
    let line = |l: PttLine| match l {
        PttLine::Rts => "rts",
        PttLine::Dtr => "dtr",
    };
    let cmd = |method: &str, device: String, model: i32, line: &str, invert: &str| TelemetryCmd::SetPttConfig {
        method: method.into(),
        device,
        model,
        speed: 0,
        line: line.into(),
        invert: invert.into(),
        gpio: p.cm108_gpio,
    };
    // Through a rigctld whenever one runs: it holds the CAT port, and on
    // Windows only one program may open a COM port.
    let rigctld = match cfg.freq_control {
        FreqControl::ManagedRigctld | FreqControl::CatFile => Some(managed.to_string()),
        FreqControl::ExternalRigctld => Some(format!("{}:{}", cfg.external.host, cfg.external.port)),
        FreqControl::None | FreqControl::Flrig => None,
    };
    match (p.method, rigctld) {
        (PttMethod::LeaveMercury, _) => None,
        (PttMethod::NoPtt, _) => Some(cmd("none", String::new(), 0, "rts", "none")),
        // CM108 is a HID device, not the COM port rigctld holds: Mercury
        // drives it with the chosen GPIO whatever runs the CAT (§5.2).
        (PttMethod::Cm108, _) => Some(cmd("cm108", String::new(), 0, "rts", "none")),
        (PttMethod::Cat | PttMethod::SerialLine, Some(addr)) => Some(cmd("hamlib", addr, 2, "rts", "none")),
        (PttMethod::SerialLine, None) => {
            let port = if p.ptt_port.trim().is_empty() { &cfg.cat.serial_port } else { &p.ptt_port };
            let invert = if p.invert { line(p.line) } else { "none" };
            Some(cmd("serial", port.trim().to_string(), 0, line(p.line), invert))
        }
        // Refused by validate(): CAT PTT needs a rigctld.
        (PttMethod::Cat, None) => None,
    }
}

/// The PTT a built-in Mercury's ini starts with (M5 §5.3): the choice
/// [`mercury_ptt_for`] sends at runtime, with the profile's own rigctld.
/// "Leave Mercury's setting" is no PTT: the private ini has no other.
pub fn builtin_mercury_ptt(cfg: &RigConfig, rigctld_port: u16) -> crate::app::builtin::MercuryPtt {
    use crate::app::builtin::MercuryPtt;
    let listen = SocketAddr::from(([127, 0, 0, 1], rigctld_port));
    let Some(TelemetryCmd::SetPttConfig { method, device, line, .. }) = mercury_ptt_for(cfg, listen) else {
        return MercuryPtt::None;
    };
    match method.as_str() {
        "hamlib" => match device.parse::<SocketAddr>() {
            Ok(a) if a.ip().is_loopback() => MercuryPtt::Rigctld { port: a.port() },
            _ => MercuryPtt::None,
        },
        "serial" => MercuryPtt::Serial { port: device, line: if line == "dtr" { PttLine::Dtr } else { PttLine::Rts } },
        "cm108" => MercuryPtt::Cm108 { device },
        _ => MercuryPtt::None,
    }
}

/// Mercury's `radio_list` already says what `want` would set.
pub fn ptt_echo_matches(rl: &RadioList, want: &TelemetryCmd) -> bool {
    let TelemetryCmd::SetPttConfig { method, device, model, line, invert, gpio, .. } = want else { return false };
    let norm = |a: &str| a.trim().replace("localhost", "127.0.0.1");
    if rl.ptt_method != *method {
        return false;
    }
    match method.as_str() {
        "hamlib" => rl.selected_model == Some(*model) && norm(&rl.device_path) == norm(device),
        "serial" => rl.device_path.trim() == device.trim() && rl.ptt_line == *line && rl.ptt_invert == *invert,
        "cm108" => rl.cm108_gpio == Some(i32::from(*gpio)),
        _ => true,
    }
}

/// M3b's stored form (`arrangement`, `host`, `port`), converted (M3b.1 §8).
fn from_legacy(v: &serde_json::Value) -> Option<RigConfig> {
    let arrangement = v.get("arrangement")?.as_str()?;
    let mut c = RigConfig {
        restore_on_startup: v.get("restoreOnStartup").and_then(|b| b.as_bool()).unwrap_or(false),
        ..RigConfig::default()
    };
    if arrangement == "rigctld" {
        c.freq_control = FreqControl::ExternalRigctld;
        if let Some(h) = v.get("host").and_then(|h| h.as_str()) {
            c.external.host = h.to_string();
        }
        if let Some(p) = v.get("port").and_then(|p| p.as_u64()).and_then(|p| u16::try_from(p).ok()) {
            c.external.port = p;
        }
        c.ptt.method = PttMethod::Cat;
    }
    Some(c)
}

impl RigConfig {
    /// The profile's stored settings (M3b's form converted), or the defaults.
    pub fn load(store: &Store, profile_id: &str) -> RigConfig {
        let Some(v) = store
            .setting(&rig_key(profile_id, "config"))
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        else {
            return RigConfig::default();
        };
        from_legacy(&v).or_else(|| serde_json::from_value(v).ok()).unwrap_or_default()
    }

    pub fn save(&self, store: &mut Store, profile_id: &str) -> Result<(), String> {
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        store.set_setting(&rig_key(profile_id, "config"), &json).map_err(|e| e.to_string())
    }

    /// A connect-only rigctld at `host:port`; Mercury's PTT left as it is.
    pub fn external(host: &str, port: u16) -> RigConfig {
        RigConfig {
            freq_control: FreqControl::ExternalRigctld,
            external: Endpoint { host: host.into(), port },
            ..RigConfig::default()
        }
    }

    /// Every field that is out of range, by the UI's field name.
    pub fn validate(&self) -> Vec<FieldError> {
        let mut errs = Vec::new();
        if !READ_EVERY_CHOICES.contains(&self.read_every_s) {
            errs.push(field("readEverySeconds", "Read the frequency every 1, 2, 5 or 10 seconds"));
        }
        if self.offset_hz.unsigned_abs() > OFFSET_LIMIT_HZ {
            errs.push(field("offsetHz", "The offset must be within ±10 000 000 Hz"));
        }
        if !(7..=8).contains(&self.cat.data_bits) {
            errs.push(field("dataBits", "Data bits must be 7 or 8"));
        }
        if !(1..=2).contains(&self.cat.stop_bits) {
            errs.push(field("stopBits", "Stop bits must be 1 or 2"));
        }
        if self.cat.wait_ms > 1000 {
            errs.push(field("waitMs", "The wait time must be 0 to 1000 ms"));
        }
        if !(1..=4).contains(&self.ptt.cm108_gpio) {
            errs.push(field("cm108Gpio", "The CM108 GPIO pin must be 1 to 4"));
        }
        if self.ptt.method == PttMethod::Cat && matches!(self.freq_control, FreqControl::None | FreqControl::Flrig) {
            errs.push(field("pttMethod", "CAT PTT needs a rigctld: choose MercuryChat's or an external rigctld"));
        }
        if self.freq_control == FreqControl::CatFile {
            if self.cat.radio.trim().is_empty() {
                errs.push(field("radio", "Choose your radio"));
            }
            if self.cat.serial_port.trim().is_empty() {
                errs.push(field("serialPort", "Enter the radio's COM port, e.g. COM3"));
            }
        }
        if self.freq_control == FreqControl::ManagedRigctld {
            if self.cat.model.is_none() {
                errs.push(field("model", "Choose your radio's model"));
            }
            match self.cat.link {
                CatLink::Serial if self.cat.serial_port.trim().is_empty() => {
                    errs.push(field("serialPort", "Enter the radio's COM port, e.g. COM3"))
                }
                CatLink::Tcp if self.cat.tcp_addr.trim().is_empty() => {
                    errs.push(field("tcpAddr", "Enter the radio's address, e.g. 192.168.1.20:50001"))
                }
                _ => {}
            }
        }
        errs
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
