//! The built-in Mercury's private ini (M5 §5.3). MercuryChat owns the keys
//! that keep the modem private (loopback only, no web origins, no store), the
//! ports and the PTT; everything else is Mercury's own (it saves the sound
//! card into this file) and is kept as Mercury wrote it.

use super::MercuryPtt;
use crate::app::PttLine;
use crate::profile::BuiltInPorts;
use std::path::{Path, PathBuf};

/// One `[section]` and its lines, as read (the preamble has the name "").
struct Section {
    name: String,
    lines: Vec<String>,
}

fn parse(text: &str) -> Vec<Section> {
    let mut out = vec![Section { name: String::new(), lines: Vec::new() }];
    for line in text.lines() {
        let t = line.trim();
        if let Some(name) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            out.push(Section { name: name.trim().to_string(), lines: Vec::new() });
        } else {
            out.last_mut().expect("a section").lines.push(line.to_string());
        }
    }
    out
}

fn key_of(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.starts_with([';', '#']) {
        return None;
    }
    t.split_once('=').map(|(k, _)| k.trim())
}

/// Set `key = value` in `[section]`: any earlier copies go, the new line goes
/// after the section's last non-blank line.
fn set(doc: &mut Vec<Section>, section: &str, key: &str, value: &str) {
    let i = match doc.iter().position(|s| s.name == section) {
        Some(i) => i,
        None => {
            if let Some(last) = doc.last_mut() {
                if last.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    last.lines.push(String::new());
                }
            }
            doc.push(Section { name: section.to_string(), lines: Vec::new() });
            doc.len() - 1
        }
    };
    let lines = &mut doc[i].lines;
    lines.retain(|l| key_of(l) != Some(key));
    let at = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(0, |p| p + 1);
    lines.insert(at, format!("{key} = {value}"));
}

/// A string value as Mercury's cfg_write quotes it.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn render(doc: &[Section]) -> String {
    let mut out = String::new();
    for s in doc {
        if !s.name.is_empty() {
            out.push_str(&format!("[{}]\n", s.name));
        }
        for l in &s.lines {
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// Write `<dir>/mercury.ini` for a built-in start, keeping the previous copy
/// as `mercury.ini.bak` and what Mercury saved there (sound card, `[audio]`).
pub fn write_ini(dir: &Path, ports: &BuiltInPorts, ptt: &MercuryPtt) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("mercury.ini");
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let mut doc = parse(old.as_deref().unwrap_or(""));

    set(&mut doc, "main", "ui_enabled", "true");
    set(&mut doc, "main", "ui_port", &ports.ui.to_string());
    set(&mut doc, "main", "arq_tcp_base_port", &ports.control.to_string());
    set(&mut doc, "main", "broadcast_tcp_port", &ports.broadcast.to_string());
    set(&mut doc, "main", "bind_address", "127.0.0.1");
    set(&mut doc, "main", "ui_allowed_origins", "\"\"");
    match ptt {
        MercuryPtt::None => set(&mut doc, "ptt", "method", "none"),
        MercuryPtt::Rigctld { port } => {
            set(&mut doc, "ptt", "method", "hamlib");
            // Hamlib model 2: NET rigctl, the managed rigctld on loopback.
            set(&mut doc, "ptt", "hamlib_model", "2");
            set(&mut doc, "ptt", "device", &quoted(&format!("127.0.0.1:{port}")));
        }
        MercuryPtt::Serial { port, line } => {
            set(&mut doc, "ptt", "method", "serial");
            set(&mut doc, "ptt", "device", &quoted(port));
            set(&mut doc, "ptt", "line", if *line == PttLine::Dtr { "dtr" } else { "rts" });
        }
        MercuryPtt::Cm108 { device } => {
            set(&mut doc, "ptt", "method", "cm108");
            set(&mut doc, "ptt", "device", &quoted(device));
        }
    }
    set(&mut doc, "store", "enabled", "false");

    if let Some(old) = &old {
        std::fs::write(dir.join("mercury.ini.bak"), old)?;
    }
    let tmp = dir.join("mercury.ini.tmp");
    std::fs::write(&tmp, render(&doc))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}
