//! The saved appearance: theme, its resolved colours, custom themes, density
//! and font scale, kept as a small JSON file in the data root
//! (`appearance.json`, written atomically like `window.json`). It lives
//! outside the core's settings store because the splash needs the theme
//! before the station data is open.
//!
//! The shell does not know the token list: it checks the shape and bounds
//! (sizes, counts, names made of safe characters, every colour a hex colour),
//! so whatever it hands back to the page is data, never markup or CSS.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const FILE: &str = "appearance.json";
/// Nothing larger is read or written.
pub const MAX_BYTES: usize = 64 * 1024;
const MAX_TOKENS: usize = 48;
const MAX_CUSTOM: usize = 24;
const MAX_NAME_CHARS: usize = 40;
const MAX_ID: usize = 48;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Custom {
    pub id: String,
    pub name: String,
    pub base: String,
    pub scheme: String,
    pub tokens: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    pub version: u32,
    /// A preset id, `system`, or a custom theme's id.
    pub theme: String,
    /// `dark` or `light`: the scheme of the resolved tokens.
    pub scheme: String,
    /// The active theme's resolved colours, for the splash.
    pub tokens: BTreeMap<String, String>,
    pub density: String,
    /// Percent, 90 to 130.
    pub font_scale: u32,
    #[serde(default)]
    pub custom: Vec<Custom>,
}

fn is_hex_colour(v: &str) -> bool {
    let digits = v.strip_prefix('#').unwrap_or("");
    matches!(digits.len(), 3 | 6 | 8) && digits.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_slug(v: &str, max: usize) -> bool {
    !v.is_empty() && v.len() <= max && v.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn check_tokens(tokens: &BTreeMap<String, String>) -> Result<(), String> {
    if tokens.len() > MAX_TOKENS {
        return Err("too many colour tokens".into());
    }
    for (k, v) in tokens {
        if !is_slug(k, 24) {
            return Err(format!("bad token name {k:?}"));
        }
        if !is_hex_colour(v) {
            return Err(format!("token {k} is not a hex colour"));
        }
    }
    Ok(())
}

/// Whether `a` is something the UI can safely be given back.
pub fn validate(a: &Appearance) -> Result<(), String> {
    if a.version != 1 {
        return Err("unsupported appearance version".into());
    }
    if !is_slug(&a.theme, MAX_ID) {
        return Err("bad theme id".into());
    }
    if !matches!(a.scheme.as_str(), "dark" | "light") {
        return Err("bad colour scheme".into());
    }
    if !matches!(a.density.as_str(), "normal" | "compact") {
        return Err("bad density".into());
    }
    if !(90..=130).contains(&a.font_scale) {
        return Err("font scale must be 90 to 130 percent".into());
    }
    check_tokens(&a.tokens)?;
    if a.custom.len() > MAX_CUSTOM {
        return Err("too many custom themes".into());
    }
    for c in &a.custom {
        if !is_slug(&c.id, MAX_ID) || !is_slug(&c.base, MAX_ID) {
            return Err("bad custom theme id".into());
        }
        if c.name.trim().is_empty() || c.name.chars().count() > MAX_NAME_CHARS || c.name.chars().any(char::is_control) {
            return Err("bad custom theme name".into());
        }
        if !matches!(c.scheme.as_str(), "dark" | "light") {
            return Err("bad custom theme scheme".into());
        }
        check_tokens(&c.tokens)?;
    }
    Ok(())
}

/// Parse and validate saved text (size-capped).
pub fn parse(text: &str) -> Result<Appearance, String> {
    if text.len() > MAX_BYTES {
        return Err("appearance data is too large".into());
    }
    let a: Appearance = serde_json::from_str(text).map_err(|e| format!("not valid appearance data: {e}"))?;
    validate(&a)?;
    Ok(a)
}

/// The saved appearance as validated JSON; a missing, oversized or corrupt file is no appearance (defaults).
pub fn load(dir: &Path) -> Option<String> {
    let path = dir.join(FILE);
    if std::fs::metadata(&path).ok()?.len() > MAX_BYTES as u64 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let a = parse(&text).ok()?;
    serde_json::to_string(&a).ok()
}

/// Validate and write atomically (temp file, then rename).
pub fn save(dir: &Path, text: &str) -> Result<(), String> {
    let a = parse(text)?;
    let out = serde_json::to_string(&a).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, out).map_err(|e| format!("cannot save appearance: {e}"))?;
    std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| format!("cannot save appearance: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        r##"{"version":1,"theme":"night-red","scheme":"dark","tokens":{"canvas":"#060000","scrim":"#000000a0"},"density":"compact","fontScale":110,"custom":[{"id":"c-1","name":"Mine","base":"ocean","scheme":"dark","tokens":{"accent":"#ff00aa"}}]}"##.into()
    }

    #[test]
    fn round_trips_through_the_data_root() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &sample()).unwrap();
        assert!(!dir.path().join("appearance.json.tmp").exists(), "the temp file is renamed away");
        let back = parse(&load(dir.path()).unwrap()).unwrap();
        assert_eq!(back.theme, "night-red");
        assert_eq!(back.font_scale, 110);
        assert_eq!(back.custom[0].tokens["accent"], "#ff00aa");
    }

    #[test]
    fn missing_or_corrupt_file_is_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).is_none());
        std::fs::write(dir.path().join(FILE), "{ not json").unwrap();
        assert!(load(dir.path()).is_none());
        std::fs::write(dir.path().join(FILE), vec![b' '; MAX_BYTES + 1]).unwrap();
        assert!(load(dir.path()).is_none(), "an oversized file is not read");
    }

    #[test]
    fn rejects_what_could_reach_css() {
        let bad = |from: &str, to: &str| {
            let text = sample().replace(from, to);
            assert!(parse(&text).is_err(), "{to} should be refused");
        };
        bad("#060000", "red; } body {");
        bad("#060000", "url(x)");
        bad("#060000", "#12");
        bad("\"canvas\"", "\"can vas\"");
        bad("\"canvas\"", "\"CANVAS\"");
        bad("\"night-red\"", "\"a;b\"");
        bad("\"dark\"", "\"neon\"");
        bad("\"compact\"", "\"huge\"");
        bad("110", "131");
        bad("110", "89");
        bad("\"Mine\"", "\"\"");
        bad("\"version\":1", "\"version\":2");
    }

    #[test]
    fn save_refuses_oversized_and_bad_data_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &sample()).unwrap();
        assert!(save(dir.path(), &" ".repeat(MAX_BYTES + 1)).is_err());
        assert!(save(dir.path(), "{}").is_err());
        assert!(load(dir.path()).is_some(), "the earlier file is intact");
    }

    #[test]
    fn counts_are_bounded() {
        let mut a = parse(&sample()).unwrap();
        for i in 0..MAX_CUSTOM {
            a.custom.push(Custom { id: format!("x-{i}"), ..a.custom[0].clone() });
        }
        assert!(validate(&a).is_err());
    }
}
