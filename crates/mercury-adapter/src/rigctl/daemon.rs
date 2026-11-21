//! MercuryChat's own rigctld (M3b.1 design §5.3): its command line, Hamlib's
//! list of rig models, and (Task 3) the process that runs it.

use std::net::SocketAddr;

/// What MercuryChat starts rigctld with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigctldSpec {
    /// Hamlib rig model (`-m`).
    pub model: u32,
    /// The radio: a serial port (`COM3`, `/dev/ttyUSB0`) or a network radio's `host:port` (`-r`).
    pub rig_path: String,
    /// Serial speed (`-s`); none for a network radio.
    pub baud: Option<u32>,
    /// Hamlib configuration parameters (`-C name=value,...`).
    pub conf: Vec<(String, String)>,
    /// PTT type (`-P`): RIG, DTR, RTS, CM108 or NONE.
    pub ptt_type: &'static str,
    /// A separate PTT device (`-p`); none: the rig's own port.
    pub ptt_path: Option<String>,
    /// Where rigctld listens (`-T`, `-t`).
    pub listen: SocketAddr,
}

/// rigctld's arguments for `spec`, in a fixed order.
pub fn rigctld_args(spec: &RigctldSpec) -> Vec<String> {
    let mut a = vec!["-m".to_string(), spec.model.to_string(), "-r".to_string(), spec.rig_path.clone()];
    if let Some(b) = spec.baud {
        a.extend(["-s".to_string(), b.to_string()]);
    }
    if !spec.conf.is_empty() {
        let c: Vec<String> = spec.conf.iter().map(|(k, v)| format!("{k}={v}")).collect();
        a.extend(["-C".to_string(), c.join(",")]);
    }
    a.extend(["-P".to_string(), spec.ptt_type.to_string()]);
    if let Some(p) = &spec.ptt_path {
        a.extend(["-p".to_string(), p.clone()]);
    }
    a.extend(["-T".to_string(), spec.listen.ip().to_string(), "-t".to_string(), spec.listen.port().to_string()]);
    a
}

/// One rig model Hamlib supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HamlibModel {
    pub id: u32,
    pub mfg: String,
    pub model: String,
    pub status: String,
}

/// `rigctld -l`. Hamlib prints fixed-width columns under a header (`Rig #`,
/// `Mfg`, `Model`, `Version`, `Status`, `Macro`), and names contain spaces,
/// so each row is cut at the header's column starts.
pub fn parse_model_list(stdout: &str) -> Vec<HamlibModel> {
    let mut lines = stdout.lines();
    let cols = loop {
        let Some(l) = lines.next() else { return Vec::new() };
        if !l.contains("Rig #") {
            continue;
        }
        let at = |name: &str| l.find(name);
        match (at("Mfg"), at("Model"), at("Version"), at("Status")) {
            (Some(m), Some(mo), Some(v), Some(s)) if m < mo && mo < v && v < s => break (m, mo, v, s),
            _ => return Vec::new(),
        }
    };
    let (mfg, model, version, status) = cols;
    let cut = |l: &str, from: usize, to: usize| -> String {
        let b = l.as_bytes();
        let from = from.min(b.len());
        let to = to.min(b.len());
        String::from_utf8_lossy(&b[from..to]).trim().to_string()
    };
    lines
        .filter_map(|l| {
            let id = cut(l, 0, mfg).parse().ok()?;
            // The status is one word; the macro name follows it.
            let rest = cut(l, status, usize::MAX);
            Some(HamlibModel {
                id,
                mfg: cut(l, mfg, model),
                model: cut(l, model, version),
                status: rest.split_whitespace().next().unwrap_or("").to_string(),
            })
        })
        .collect()
}
