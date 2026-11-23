//! The diagnostics file (M5 §7): what the owner needs to see why something
//! went wrong, and nothing of the operator's traffic. It holds the app's logs
//! (they never hold message bodies, OPS-01) and the last crash report, the
//! tails of Mercury's and rigctld's logs, the versions, and the station's
//! settings without its email or any password. Never a message, a file, or
//! the database. The window lists the entries before anything is saved, and
//! the file holds exactly that list.

use crate::profile::StationProfile;
use serde::Serialize;
use std::io::Write;
use std::path::Path;
use ts_rs::TS;

/// Lines kept from the end of Mercury's and rigctld's logs.
pub const LOG_TAIL_LINES: usize = 2000;

/// Where the entries come from, gathered by the service.
pub struct Sources<'a> {
    /// The app's log folder (`mercurychat.log.*`, `crash-*.txt`).
    pub logs_dir: Option<&'a Path>,
    /// The built-in Mercury's log, for a built-in station.
    pub mercury_log: Option<&'a Path>,
    /// MercuryChat's rigctld's output and the rig link's errors, as kept.
    pub rig_log: &'a [String],
    pub profile: Option<&'a StationProfile>,
    pub versions: &'a Versions,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Versions {
    pub app: String,
    pub mercury: String,
    pub hamlib: String,
    pub os: String,
}

/// One file in the zip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagEntry {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// An entry as the window lists it before saving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiagEntryView {
    pub name: String,
    #[ts(type = "number")]
    pub bytes: u64,
}

impl From<&DiagEntry> for DiagEntryView {
    fn from(e: &DiagEntry) -> Self {
        DiagEntryView { name: e.name.clone(), bytes: e.bytes.len() as u64 }
    }
}

/// The last `n` lines of `text`, each ending in a newline.
fn tail(lines: impl DoubleEndedIterator<Item = String>, n: usize) -> Vec<u8> {
    let mut kept: Vec<String> = lines.rev().take(n).collect();
    kept.reverse();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.into_bytes()
}

/// The entries, in the order they are listed and written: versions, the
/// settings, the app's logs (oldest first), the last crash report, then the
/// two log tails. Unreadable sources are left out, never guessed.
pub fn collect(src: &Sources) -> Vec<DiagEntry> {
    let mut out = Vec::new();
    let v = src.versions;
    let versions = format!(
        "MercuryChat {}\nMercury {}\nHamlib {}\nOS {}\n",
        v.app,
        if v.mercury.is_empty() { "(not built in)" } else { &v.mercury },
        if v.hamlib.is_empty() { "(not built in)" } else { &v.hamlib },
        v.os
    );
    out.push(DiagEntry { name: "versions.txt".into(), bytes: versions.into_bytes() });
    if let Some(p) = src.profile {
        if let Ok(v) = serde_json::to_value(p) {
            let json = serde_json::to_string_pretty(&scrub(v)).unwrap_or_default();
            out.push(DiagEntry { name: "profile.json".into(), bytes: json.into_bytes() });
        }
    }
    if let Some(dir) = src.logs_dir {
        let mut logs = Vec::new();
        let mut crashes = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            if name.starts_with("mercurychat.log") {
                logs.push(name);
            } else if name.starts_with("crash-") && name.ends_with(".txt") {
                crashes.push(name);
            }
        }
        logs.sort();
        crashes.sort();
        for name in logs {
            if let Ok(bytes) = std::fs::read(dir.join(&name)) {
                out.push(DiagEntry { name: format!("logs/{name}"), bytes });
            }
        }
        // The names carry a UTC time stamp: the last sorts last.
        if let Some(bytes) = crashes.last().and_then(|c| std::fs::read(dir.join(c)).ok()) {
            out.push(DiagEntry { name: "crash.txt".into(), bytes });
        }
    }
    if let Some(text) = src.mercury_log.and_then(|p| std::fs::read(p).ok()) {
        let text = String::from_utf8_lossy(&text).into_owned();
        out.push(DiagEntry { name: "mercury.log".into(), bytes: tail(text.lines().map(String::from), LOG_TAIL_LINES) });
    }
    out.push(DiagEntry { name: "rigctld.log".into(), bytes: tail(src.rig_log.iter().cloned(), LOG_TAIL_LINES) });
    out
}

/// The settings without `email` or any field whose name holds `password`, at any depth.
pub fn scrub(v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(k, _)| {
                    let k = k.to_ascii_lowercase();
                    k != "email" && !k.contains("password")
                })
                .map(|(k, v)| (k, scrub(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(scrub).collect()),
        other => other,
    }
}

/// Write the entries, in order, as a zip at `path` (replaced if it exists).
pub fn write_zip(entries: &[DiagEntry], path: &Path) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for e in entries {
        zip.start_file(e.name.as_str(), opts).map_err(std::io::Error::other)?;
        zip.write_all(&e.bytes)?;
    }
    zip.finish().map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::StationProfile;
    use std::io::Read;

    const SECRET: &str = "the planted message body 7f3a";

    fn versions() -> Versions {
        Versions { app: "0.5.0".into(), mercury: "1.9.25".into(), hamlib: "4.7.2".into(), os: "windows x86_64".into() }
    }

    /// A data folder as the app leaves it: logs, a crash report, the
    /// database, a received file and a chat export beside them.
    fn data(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let logs = root.join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("mercurychat.log.2026-11-22"), "INFO attached\n").unwrap();
        std::fs::write(logs.join("mercurychat.log.2026-11-23"), "WARN audio\n").unwrap();
        std::fs::write(logs.join("crash-20261120T000000Z.txt"), "old crash\n").unwrap();
        std::fs::write(logs.join("crash-20261123T101500Z.txt"), "panicked at x\n").unwrap();
        std::fs::write(logs.join("notes.txt"), SECRET).unwrap();
        std::fs::write(root.join("mercurychat.db"), SECRET).unwrap();
        std::fs::write(root.join("station.db"), SECRET).unwrap();
        let files = root.join("stations").join("p1").join("files");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("antenna.jpg"), SECRET).unwrap();
        let mercury = root.join("stations").join("p1").join("mercury");
        std::fs::create_dir_all(&mercury).unwrap();
        let mut log = String::new();
        for i in 1..=2500 {
            log.push_str(&format!("mercury line {i}\n"));
        }
        std::fs::write(mercury.join("mercury.log"), log).unwrap();
        (logs, mercury.join("mercury.log"))
    }

    fn profile() -> StationProfile {
        let mut p = StationProfile::new_builtin("ZS6ABC");
        p.name = Some("Alex".into());
        p.email = Some("alex@example.org".into());
        p
    }

    fn zip_names_and_text(path: &Path) -> Vec<(String, String)> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        (0..z.len())
            .map(|i| {
                let mut f = z.by_index(i).unwrap();
                let mut s = String::new();
                f.read_to_string(&mut s).unwrap();
                (f.name().map(|n| n.to_string()).unwrap(), s)
            })
            .collect()
    }

    fn entries(root: &Path, rig: &[String]) -> Vec<DiagEntry> {
        let (logs, mercury) = data(root);
        let p = profile();
        let v = versions();
        collect(&Sources {
            logs_dir: Some(&logs),
            mercury_log: Some(&mercury),
            rig_log: rig,
            profile: Some(&p),
            versions: &v,
        })
    }

    #[test]
    fn no_message_body_file_or_db() {
        let dir = tempfile::tempdir().unwrap();
        let e = entries(dir.path(), &["rigctld: ready".into()]);
        let zip = dir.path().join("out.zip");
        write_zip(&e, &zip).unwrap();
        let all = zip_names_and_text(&zip);
        let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "versions.txt",
                "profile.json",
                "logs/mercurychat.log.2026-11-22",
                "logs/mercurychat.log.2026-11-23",
                "crash.txt",
                "mercury.log",
                "rigctld.log"
            ]
        );
        for (n, text) in &all {
            assert!(!text.contains(SECRET), "{n} holds the planted message");
            assert!(!n.ends_with(".db") && !n.contains("antenna"), "{n}");
        }
        let crash = &all.iter().find(|(n, _)| n == "crash.txt").unwrap().1;
        assert_eq!(crash, "panicked at x\n", "the last crash report only");
        let versions = &all[0].1;
        for want in ["MercuryChat 0.5.0", "Mercury 1.9.25", "Hamlib 4.7.2", "windows x86_64"] {
            assert!(versions.contains(want), "{versions}");
        }
    }

    #[test]
    fn email_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let e = entries(dir.path(), &[]);
        let json = String::from_utf8(e.iter().find(|e| e.name == "profile.json").unwrap().bytes.clone()).unwrap();
        assert!(json.contains("ZS6ABC") && json.contains("Alex"), "{json}");
        assert!(!json.contains("alex@example.org") && !json.contains("\"email\""), "{json}");
        let v: serde_json::Value =
            serde_json::json!({"a": {"rigPassword": "x", "Password2": "y", "keep": 1}, "email": "e"});
        let scrubbed = scrub(v);
        assert_eq!(scrubbed, serde_json::json!({"a": {"keep": 1}}));
    }

    #[test]
    fn log_tails_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let rig: Vec<String> = (1..=3000).map(|i| format!("rig line {i}")).collect();
        let e = entries(dir.path(), &rig);
        let text = |name: &str| String::from_utf8(e.iter().find(|e| e.name == name).unwrap().bytes.clone()).unwrap();
        let mercury = text("mercury.log");
        assert_eq!(mercury.lines().count(), LOG_TAIL_LINES);
        assert!(
            mercury.starts_with("mercury line 501\n") && mercury.ends_with("mercury line 2500\n"),
            "the last lines"
        );
        let rig = text("rigctld.log");
        assert_eq!(rig.lines().count(), LOG_TAIL_LINES);
        assert!(rig.ends_with("rig line 3000\n"));
    }

    #[test]
    fn preview_lists_exactly_what_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let e = entries(dir.path(), &["rigctld: ready".into()]);
        let preview: Vec<DiagEntryView> = e.iter().map(DiagEntryView::from).collect();
        let zip = dir.path().join("out.zip");
        write_zip(&e, &zip).unwrap();
        let written = zip_names_and_text(&zip);
        assert_eq!(preview.len(), written.len());
        for (p, (n, text)) in preview.iter().zip(&written) {
            assert_eq!(&p.name, n);
            assert_eq!(p.bytes, text.len() as u64, "{n}");
        }
    }
}
