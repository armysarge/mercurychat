//! The built-in modem's shipped programs (M5 design G4): MercuryChat checks a
//! program's SHA-256 against the manifest compiled into the app before it
//! starts it, and refuses a changed, missing or unlisted one. The versions
//! shown next to "Mercury" and "Hamlib" in the window come from the same
//! build (owner, 23 Nov 2026).
//!
//! The manifest is the bundle's `MANIFEST.sha256` (`<sha256 hex>  <path>` per
//! line); `BUILD.txt` holds `key=value` lines (`mercury_version`,
//! `hamlib_version`, …). Release builds compile both in (src-tauri/build.rs);
//! a development build may point `MC_BUNDLE_DIR` at a lab bundle instead.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The bundled Mercury and rigctld, as the manifest names them.
pub const MERCURY_EXE: &str = if cfg!(windows) { "mercury.exe" } else { "mercury" };
pub const RIGCTLD_EXE: &str = if cfg!(windows) { "rigctld.exe" } else { "rigctld" };

/// The shipped programs, where they are, and what they must hash to.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub dir: PathBuf,
    manifest: &'static str,
    build: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// Listed, but not on disk.
    Missing(String),
    /// On disk, but not what was shipped.
    Changed(String),
    /// Not a file the manifest lists (or not a plain relative name).
    NotInManifest(String),
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = |name: &str| if name.starts_with("rigctld") { "Hamlib's rigctld" } else { "Mercury" };
        match self {
            BundleError::Changed(n) => {
                write!(f, "This copy of {} has been changed. Reinstall MercuryChat.", what(n))
            }
            BundleError::Missing(n) => {
                write!(f, "{n} is missing from MercuryChat's modem folder. Reinstall MercuryChat.")
            }
            BundleError::NotInManifest(n) => write!(f, "{n} is not part of this MercuryChat build."),
        }
    }
}

impl std::error::Error for BundleError {}

/// The built-in Mercury and Hamlib versions, as built.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BundleVersions {
    pub mercury: String,
    pub hamlib: String,
}

impl Bundle {
    /// The bundle a release build ships, or `None` when this build has none
    /// (an empty manifest: a development build without a bundle).
    pub fn shipped(dir: PathBuf, manifest: &'static str, build: &'static str) -> Option<Bundle> {
        if manifest.trim().is_empty() {
            return None;
        }
        Some(Bundle { dir, manifest, build })
    }

    /// A lab bundle in a development build (`MC_BUNDLE_DIR`): its own
    /// `MANIFEST.sha256` and `BUILD.txt`, read once.
    pub fn from_dir(dir: PathBuf) -> std::io::Result<Bundle> {
        let manifest = std::fs::read_to_string(dir.join("MANIFEST.sha256"))?;
        let build = std::fs::read_to_string(dir.join("BUILD.txt")).unwrap_or_default();
        Ok(Bundle { dir, manifest: Box::leak(manifest.into_boxed_str()), build: Box::leak(build.into_boxed_str()) })
    }

    pub fn for_tests(dir: PathBuf, manifest: &'static str, build: &'static str) -> Bundle {
        Bundle { dir, manifest, build }
    }

    /// The path of `rel` (e.g. `mercury.exe`) once its SHA-256 matches the manifest.
    pub fn verify(&self, rel: &str) -> Result<PathBuf, BundleError> {
        let unsafe_name = rel.is_empty()
            || rel.starts_with(['/', '\\'])
            || rel.contains(':')
            || rel.split(['/', '\\']).any(|p| p == "..");
        let want = (!unsafe_name)
            .then(|| self.manifest.lines().find_map(|l| l.split_once("  ").filter(|(_, n)| n.trim() == rel)))
            .flatten()
            .map(|(sha, _)| sha.trim().to_ascii_lowercase())
            .ok_or_else(|| BundleError::NotInManifest(rel.to_string()))?;
        let path = self.dir.join(rel);
        let got = sha256_file(&path).map_err(|_| BundleError::Missing(rel.to_string()))?;
        if got != want {
            return Err(BundleError::Changed(rel.to_string()));
        }
        Ok(path)
    }

    /// The names the manifest lists, in its order.
    pub fn manifest_names(&self) -> impl Iterator<Item = &str> {
        self.manifest.lines().filter_map(|l| l.split_once("  ").map(|(_, n)| n.trim())).filter(|n| !n.is_empty())
    }

    /// Every file the manifest lists, checked: the first one that fails.
    pub fn verify_all(&self) -> Result<(), BundleError> {
        for name in self.manifest_names() {
            self.verify(name)?;
        }
        Ok(())
    }

    pub fn versions(&self) -> BundleVersions {
        let get = |key: &str| {
            self.build
                .lines()
                .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
                .map(|v| v.trim().trim_start_matches('v').to_string())
                .unwrap_or_default()
        };
        BundleVersions { mercury: get("mercury_version"), hamlib: get("hamlib_version") }
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    /// A bundle folder with `mercury` and `rigctld`, and its manifest.
    fn folder() -> (tempfile::TempDir, &'static str) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("mercury"), b"mercury program").unwrap();
        std::fs::write(d.path().join("rigctld"), b"rigctld program").unwrap();
        let line = |n: &str| format!("{}  {n}\n", sha256_file(&d.path().join(n)).unwrap());
        let m = leak(line("mercury") + &line("rigctld"));
        (d, m)
    }

    const BUILD: &str = "target=linux-amd64\nhamlib_version=4.7.2\nmercury_version=v1.9.25\n";

    #[test]
    fn verify_accepts_the_manifest_file() {
        let (d, m) = folder();
        let b = Bundle::for_tests(d.path().into(), m, BUILD);
        assert_eq!(b.verify("mercury").unwrap(), d.path().join("mercury"));
        assert!(b.verify("rigctld").is_ok());
    }

    #[test]
    fn changed_file_is_refused() {
        let (d, m) = folder();
        std::fs::write(d.path().join("mercury"), b"mercury programX").unwrap();
        let b = Bundle::for_tests(d.path().into(), m, BUILD);
        let e = b.verify("mercury").unwrap_err();
        assert_eq!(e, BundleError::Changed("mercury".into()));
        assert_eq!(e.to_string(), "This copy of Mercury has been changed. Reinstall MercuryChat.");
    }

    #[test]
    fn missing_file_is_refused() {
        let (d, m) = folder();
        std::fs::remove_file(d.path().join("rigctld")).unwrap();
        let b = Bundle::for_tests(d.path().into(), m, BUILD);
        assert_eq!(b.verify("rigctld").unwrap_err(), BundleError::Missing("rigctld".into()));
    }

    #[test]
    fn unlisted_file_is_refused() {
        let (d, m) = folder();
        std::fs::write(d.path().join("other.exe"), b"x").unwrap();
        let b = Bundle::for_tests(d.path().into(), m, BUILD);
        assert_eq!(b.verify("other.exe").unwrap_err(), BundleError::NotInManifest("other.exe".into()));
    }

    #[test]
    fn manifest_paths_cannot_escape() {
        let (d, _) = folder();
        let outside = d.path().parent().unwrap().join("x");
        std::fs::write(&outside, b"x").unwrap();
        let m = leak(format!("{}  ../x\n", sha256_file(&outside).unwrap()));
        let b = Bundle::for_tests(d.path().into(), m, BUILD);
        assert_eq!(b.verify("../x").unwrap_err(), BundleError::NotInManifest("../x".into()));
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn versions_come_from_build_txt() {
        let (d, m) = folder();
        let b = Bundle::for_tests(d.path().into(), m, BUILD);
        assert_eq!(b.versions(), BundleVersions { mercury: "1.9.25".into(), hamlib: "4.7.2".into() });
    }

    #[test]
    fn verify_all_names_the_changed_file() {
        let (d, m) = folder();
        let b = Bundle::for_tests(d.path().into(), m, BUILD);
        assert_eq!(b.verify_all(), Ok(()));
        std::fs::write(d.path().join("rigctld"), b"changed").unwrap();
        assert_eq!(b.verify_all(), Err(BundleError::Changed("rigctld".into())));
    }

    #[test]
    fn no_manifest_means_no_bundle() {
        assert!(Bundle::shipped("x".into(), "", BUILD).is_none());
        assert!(Bundle::shipped("x".into(), "  \n", BUILD).is_none());
    }
}
