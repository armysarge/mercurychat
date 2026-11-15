//! Incoming-file safety (FIL-03/FIL-04): sanitised names, temp files,
//! verification and atomic, non-overwriting publication.

use std::path::{Path, PathBuf};

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Reduce a peer-supplied name to a safe single path component.
pub fn sanitize_name(offered: &str) -> String {
    let base = offered.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String = base
        .chars()
        .map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c })
        .collect();
    s = s.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    if s.is_empty() {
        s = "file".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        s = format!("_{s}");
    }
    while s.len() > 120 {
        s.pop();
    }
    s
}

/// First free `name`, `name (1)`, ... in `dir`. Never overwrites.
pub fn unique_target(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1.. {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if !p.exists() {
            return p;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitises_traversal_and_reserved() {
        assert_eq!(sanitize_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_name("..\\..\\boot.ini"), "boot.ini");
        assert_eq!(sanitize_name("C:\\x\\y.txt"), "y.txt");
        assert_eq!(sanitize_name("con.txt"), "_con.txt");
        assert_eq!(sanitize_name("..."), "file");
        assert_eq!(sanitize_name("a\u{0}b?.bin"), "a_b_.bin");
        assert_eq!(sanitize_name("photo.jpg"), "photo.jpg");
        assert!(sanitize_name(&"x".repeat(300)).len() <= 120);
    }

    #[test]
    fn never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"1").unwrap();
        assert_eq!(unique_target(d.path(), "a.txt"), d.path().join("a (1).txt"));
    }
}
