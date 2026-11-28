//! Incoming-file safety (FIL-03/FIL-04): sanitised names, temp files,
//! verification and atomic, non-overwriting publication.

pub mod pace;
pub mod prepare;
pub mod unpack;

pub use prepare::{prepare, PhotoSize, PrepareError, Prepared};
pub use unpack::{unpack_zstd, UnpackError};

use std::path::{Path, PathBuf};

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Reduce a peer-supplied name to a safe single path component.
pub fn sanitize_name(offered: &str) -> String {
    let base = offered.rsplit(['/', '\\']).next().unwrap_or("");
    let mut s: String =
        base.chars().map(|c| if c.is_control() || is_format(c) || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
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

/// A peer-supplied name as shown to the operator: format characters, which
/// could make it read as another name (a right-to-left override turning
/// `gpj.exe` into `exe.jpg`), become `_`.
pub fn display_name(offered: &str) -> String {
    offered.chars().map(|c| if is_format(c) { '_' } else { c }).collect()
}

/// Unicode format characters (category Cf): bidi controls, zero-width
/// characters, the BOM and tag characters.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// `name`, then `name (1)`, `name (2)`, ... in `dir`.
fn candidates<'a>(dir: &'a Path, name: &'a str) -> impl Iterator<Item = PathBuf> + 'a {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    std::iter::once(dir.join(name)).chain((1u32..).map(move |n| dir.join(format!("{stem} ({n}){ext}"))))
}

/// Move `part` into `dir` under the first free name, never over anything.
pub fn publish(part: &Path, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    publish_with(part, dir, name, |from, to| std::fs::hard_link(from, to))
}

/// [`publish`] with the linking call given (tests). A hard link gives the whole
/// file its name in one step and fails if the name is taken, however recently.
/// Where the folder cannot hold links (FAT, some network folders), the name is
/// claimed by creating it, and only that empty file of ours is replaced.
fn publish_with(
    part: &Path,
    dir: &Path,
    name: &str,
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<PathBuf> {
    use std::io::ErrorKind::AlreadyExists;
    for target in candidates(dir, name) {
        // `exists` first: creating over a folder fails differently on Windows.
        if target.exists() {
            continue;
        }
        match link(part, &target) {
            Ok(()) => {
                // The file is published; a leftover `.part` name is cleared at
                // the next start.
                let _ = std::fs::remove_file(part);
                return Ok(target);
            }
            Err(e) if e.kind() == AlreadyExists => continue,
            Err(_) => {}
        }
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&target) {
            Ok(claim) => drop(claim),
            Err(e) if e.kind() == AlreadyExists => continue,
            Err(e) => return Err(e),
        }
        if let Err(e) = while_denied(|| std::fs::rename(part, &target)) {
            let _ = std::fs::remove_file(&target);
            return Err(e);
        }
        return Ok(target);
    }
    Err(std::io::Error::other("no free file name"))
}

/// Retry for up to a second while access is denied: on Windows a virus scanner
/// or sync program may briefly hold a file that has just appeared.
fn while_denied(mut op: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    for _ in 0..10 {
        match op() {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            done => return done,
        }
    }
    op()
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

    /// Final review 2: format characters (bidi overrides, zero-width) can't
    /// make a name read as something else.
    #[test]
    fn strips_bidi_and_invisible() {
        assert_eq!(sanitize_name("holiday\u{202E}gpj.exe"), "holiday_gpj.exe");
        assert_eq!(display_name("holiday\u{202E}gpj.exe"), "holiday_gpj.exe");
        assert_eq!(sanitize_name("a\u{200B}b\u{2066}c\u{FEFF}.txt"), "a_b_c_.txt");
        assert_eq!(display_name("../../evil.txt"), "../../evil.txt", "display keeps the rest as sent");
    }

    /// L4: a name free when it was chosen may be taken when the file is moved
    /// there. Nothing may be overwritten, however closely they race.
    #[test]
    fn publishing_at_once_overwrites_nothing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"already here").unwrap();
        for round in 0..20 {
            let dir = d.path().join(round.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            let start = std::sync::Barrier::new(8);
            std::thread::scope(|s| {
                for i in 0..8u8 {
                    let (dir, start) = (&dir, &start);
                    s.spawn(move || {
                        let part = dir.join(format!(".{i}.part"));
                        std::fs::write(&part, [i]).unwrap();
                        start.wait();
                        publish(&part, dir, "photo.jpg").unwrap();
                    });
                }
            });
            let mut contents: Vec<u8> =
                std::fs::read_dir(&dir).unwrap().map(|e| std::fs::read(e.unwrap().path()).unwrap()[0]).collect();
            contents.sort_unstable();
            assert_eq!(contents, [0, 1, 2, 3, 4, 5, 6, 7], "round {round}: a file was lost");
        }
    }

    /// The name appears with the whole file or not at all: no empty stand-in
    /// that a virus scanner or sync program could open and so block the move.
    #[test]
    fn a_published_name_never_shows_an_unfinished_file() {
        let d = tempfile::tempdir().unwrap();
        for round in 0..2000 {
            let part = d.path().join(format!(".{round}.part"));
            std::fs::write(&part, [7u8; 64]).unwrap();
            let target = d.path().join(format!("{round}.bin"));
            let start = std::sync::Barrier::new(2);
            let done = std::sync::atomic::AtomicBool::new(false);
            let seen = std::thread::scope(|s| {
                let watcher = s.spawn(|| {
                    start.wait();
                    loop {
                        let last = done.load(std::sync::atomic::Ordering::SeqCst);
                        if let Ok(m) = std::fs::metadata(&target) {
                            return Some(m.len());
                        }
                        if last {
                            return None;
                        }
                    }
                });
                start.wait();
                publish(&part, d.path(), &format!("{round}.bin")).unwrap();
                done.store(true, std::sync::atomic::Ordering::SeqCst);
                watcher.join().unwrap()
            });
            assert_eq!(seen, Some(64), "round {round}");
        }
    }

    #[test]
    fn publishing_steps_past_what_is_there() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"1").unwrap();
        std::fs::create_dir(d.path().join("a (1).txt")).unwrap();
        let part = d.path().join(".x.part");
        std::fs::write(&part, b"new").unwrap();
        let to = publish(&part, d.path(), "a.txt").unwrap();
        assert_eq!(to, d.path().join("a (2).txt"));
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
        assert_eq!(std::fs::read(d.path().join("a.txt")).unwrap(), b"1");
        assert!(!part.exists());
    }

    #[test]
    fn a_folder_without_links_still_publishes_without_overwriting() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), b"1").unwrap();
        let part = d.path().join(".x.part");
        std::fs::write(&part, b"new").unwrap();
        let no_links = |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
        let to = publish_with(&part, d.path(), "a.txt", no_links).unwrap();
        assert_eq!(to, d.path().join("a (1).txt"));
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
        assert_eq!(std::fs::read(d.path().join("a.txt")).unwrap(), b"1");
        assert!(!part.exists());
        assert!(publish_with(&part, d.path(), "b.txt", no_links).is_err());
        assert!(!d.path().join("b.txt").exists(), "no empty file left");
    }

    #[test]
    fn a_briefly_denied_move_is_tried_again() {
        use std::io::{Error, ErrorKind};
        let mut calls = 0;
        let held_twice = while_denied(|| {
            calls += 1;
            if calls < 3 {
                Err(Error::from(ErrorKind::PermissionDenied))
            } else {
                Ok(())
            }
        });
        assert!(held_twice.is_ok());
        assert_eq!(calls, 3);
        let mut calls = 0;
        let gone = while_denied(|| {
            calls += 1;
            Err(Error::from(ErrorKind::NotFound))
        });
        assert_eq!(gone.unwrap_err().kind(), ErrorKind::NotFound);
        assert_eq!(calls, 1, "only a denial is retried");
    }

    #[test]
    fn a_publish_that_fails_leaves_no_empty_file() {
        let d = tempfile::tempdir().unwrap();
        assert!(publish(&d.path().join("missing.part"), d.path(), "a.txt").is_err());
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    }
}
