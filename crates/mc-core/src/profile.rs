//! Station profiles (SET-01, SET-03): who the station is and which modem it
//! uses. `validate` returns the normalised profile or every field error at
//! once, worded for the operator. A stored listen preference never arms
//! transmission by itself (RF-02): the app asks before `LISTEN ON`.

use mc_storage::ProfileRow;
use mch_protocol::beacon::{normalise_grid, valid_grid};
use mercury_adapter::Callsign;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct StationProfile {
    pub id: String,
    pub callsign: String,
    pub name: Option<String>,
    pub grid: Option<String>,
    pub modem: ModemTarget,
    pub lab: bool,
    pub receive_only: bool,
    /// Station information (CHT-06): optional, at most 128 characters each,
    /// inserted into drafts by macro tags. Never sent by themselves.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub qth: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub rig: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub power: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub antenna: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub email: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub about: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ModemTarget {
    /// A Mercury the operator runs (SET-03).
    Existing {
        host: String,
        control_port: u16,
        data_port: u16,
        broadcast_port: Option<u16>,
        telemetry_url: Option<String>,
    },
    /// The in-process simulator: no audio, no RF (M1).
    Simulator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

fn err(field: &str, message: &str) -> FieldError {
    FieldError { field: field.into(), message: message.into() }
}

const NAME_MAX: usize = 64;
/// Most characters in one station-information field (CHT-06).
const INFO_MAX: usize = 128;
const HOST_MAX: usize = 253;
const TELEMETRY_URL_MAX: usize = 256;

/// Why a (trimmed, non-empty) telemetry address is refused, if it is: it must
/// be exactly `ws://host:port/path` (Mercury's plain WebSocket; no TLS, no
/// credentials), at most 256 characters.
fn telemetry_url_error(url: &str) -> Option<&'static str> {
    const SHAPE: &str = "Use ws://host:port/path, e.g. ws://127.0.0.1:10000/websocket";
    if url.chars().count() > TELEMETRY_URL_MAX {
        return Some("At most 256 characters");
    }
    if url.starts_with("wss://") {
        return Some("wss:// (TLS) is not supported: use ws://host:port/path");
    }
    let Some(rest) = url.strip_prefix("ws://") else { return Some(SHAPE) };
    let Some(slash) = rest.find('/') else { return Some(SHAPE) };
    let (authority, path) = rest.split_at(slash);
    let (host_ok, port) = match authority.strip_prefix('[') {
        // An IPv6 literal: [addr]:port.
        Some(v6) => match v6.split_once("]:") {
            Some((addr, port)) => (!addr.is_empty() && addr.bytes().all(|b| b.is_ascii_hexdigit() || b == b':'), port),
            None => return Some(SHAPE),
        },
        // A name or IPv4 address: no credentials, no other punctuation.
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (
                !host.is_empty()
                    && host.len() <= HOST_MAX
                    && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
                port,
            ),
            None => return Some(SHAPE),
        },
    };
    let port_ok =
        !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p != 0);
    let path_ok = path.bytes().all(|b| b.is_ascii_graphic());
    if host_ok && port_ok && path_ok {
        None
    } else {
        Some(SHAPE)
    }
}

/// Normalise a station-information field in place (trim; blank means none)
/// and say why it is refused, if it is.
fn station_info(value: &mut Option<String>) -> Option<&'static str> {
    *value = value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from);
    let v = value.as_deref()?;
    if v.chars().count() > INFO_MAX {
        Some("At most 128 characters")
    } else if v.chars().any(char::is_control) {
        Some("No control characters")
    } else {
        None
    }
}

impl StationProfile {
    /// An existing Mercury on this PC with Mercury's default ports.
    pub fn new_existing(callsign: &str) -> StationProfile {
        StationProfile::new(
            callsign,
            ModemTarget::Existing {
                host: "127.0.0.1".into(),
                control_port: 8300,
                data_port: 8301,
                broadcast_port: Some(8100),
                telemetry_url: None,
            },
        )
    }

    pub fn new_simulator(callsign: &str) -> StationProfile {
        StationProfile::new(callsign, ModemTarget::Simulator)
    }

    fn new(callsign: &str, modem: ModemTarget) -> StationProfile {
        StationProfile {
            id: uuid::Uuid::new_v4().to_string(),
            callsign: callsign.into(),
            name: None,
            grid: None,
            modem,
            lab: false,
            receive_only: false,
            qth: None,
            rig: None,
            power: None,
            antenna: None,
            email: None,
            about: None,
        }
    }

    pub fn validate(&self) -> Result<StationProfile, Vec<FieldError>> {
        let mut errors = Vec::new();
        let mut p = self.clone();
        // A draft from the UI has no id yet: give it one, never store "".
        if p.id.trim().is_empty() {
            p.id = uuid::Uuid::new_v4().to_string();
        }

        // Edges are trimmed (pasted input); anything inside must be a callsign.
        match Callsign::parse(self.callsign.trim()) {
            Ok(c) => p.callsign = c.as_str().to_string(),
            Err(_) => errors.push(err("callsign", "Use 1–15 letters, digits, / or -")),
        }

        p.name = self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(String::from);
        if p.name.as_ref().is_some_and(|n| n.chars().count() > NAME_MAX) {
            errors.push(err("name", "At most 64 characters"));
        }

        p.grid = match self.grid.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
            Some(g) if valid_grid(g) => Some(normalise_grid(g)),
            Some(_) => {
                errors.push(err("grid", "Use a 4, 6 or 8 character Maidenhead locator"));
                None
            }
            None => None,
        };

        if let ModemTarget::Existing { host, control_port, data_port, broadcast_port, telemetry_url } = &mut p.modem {
            *host = host.trim().to_string();
            if host.is_empty() || host.len() > HOST_MAX || host.contains(char::is_whitespace) {
                errors.push(err("host", "Enter a host name or address"));
            }
            for (field, port) in [("controlPort", Some(*control_port)), ("dataPort", Some(*data_port))]
                .into_iter()
                .chain([("broadcastPort", *broadcast_port)])
            {
                if port == Some(0) {
                    errors.push(err(field, "Port must be 1–65535"));
                }
            }
            if *data_port != 0 && data_port == control_port {
                errors.push(err("dataPort", "Must differ from the control port"));
            }
            if let Some(b) = *broadcast_port {
                if b != 0 && (b == *control_port || b == *data_port) {
                    errors.push(err("broadcastPort", "Must differ from the control and data ports"));
                }
            }
            *telemetry_url = telemetry_url.as_deref().map(str::trim).filter(|u| !u.is_empty()).map(String::from);
            if let Some(why) = telemetry_url.as_deref().and_then(telemetry_url_error) {
                errors.push(err("telemetryUrl", why));
            }
        }

        for (field, value) in [
            ("qth", &mut p.qth),
            ("rig", &mut p.rig),
            ("power", &mut p.power),
            ("antenna", &mut p.antenna),
            ("email", &mut p.email),
            ("about", &mut p.about),
        ] {
            if let Some(why) = station_info(value) {
                errors.push(err(field, why));
            }
        }

        if errors.is_empty() {
            Ok(p)
        } else {
            Err(errors)
        }
    }
}

pub fn to_row(p: &StationProfile) -> ProfileRow {
    let (kind, host, control_port, data_port, broadcast_port, telemetry_url) = match &p.modem {
        ModemTarget::Existing { host, control_port, data_port, broadcast_port, telemetry_url } => (
            "existing",
            Some(host.clone()),
            Some(*control_port),
            Some(*data_port),
            *broadcast_port,
            telemetry_url.clone(),
        ),
        ModemTarget::Simulator => ("simulator", None, None, None, None, None),
    };
    ProfileRow {
        id: p.id.clone(),
        callsign: p.callsign.clone(),
        name: p.name.clone(),
        grid: p.grid.clone(),
        modem_kind: kind.into(),
        host,
        control_port,
        data_port,
        broadcast_port,
        telemetry_url,
        lab: p.lab,
        receive_only: p.receive_only,
        qth: p.qth.clone(),
        rig: p.rig.clone(),
        power: p.power.clone(),
        antenna: p.antenna.clone(),
        email: p.email.clone(),
        about: p.about.clone(),
    }
}

pub fn from_row(r: &ProfileRow) -> Result<StationProfile, FieldError> {
    let modem = match r.modem_kind.as_str() {
        "existing" => ModemTarget::Existing {
            host: r.host.clone().unwrap_or_default(),
            control_port: r.control_port.unwrap_or(0),
            data_port: r.data_port.unwrap_or(0),
            broadcast_port: r.broadcast_port,
            telemetry_url: r.telemetry_url.clone(),
        },
        "simulator" => ModemTarget::Simulator,
        other => return Err(err("modemKind", &format!("Unknown modem kind {other:?}"))),
    };
    Ok(StationProfile {
        id: r.id.clone(),
        callsign: r.callsign.clone(),
        name: r.name.clone(),
        grid: r.grid.clone(),
        modem,
        lab: r.lab,
        receive_only: r.receive_only,
        qth: r.qth.clone(),
        rig: r.rig.clone(),
        power: r.power.clone(),
        antenna: r.antenna.clone(),
        email: r.email.clone(),
        about: r.about.clone(),
    })
}
