//! The radios MercuryChat can control itself (M5 Task 11e): the ones that
//! ship with it, and the ones the operator imported from a CAT commands file.
//! Imported radios are kept in the app's settings in the app's own format.

use mc_storage::Store;
use mercury_adapter::cat::file::{self, Radio};

const IMPORTED: &str = "cat.radios";
/// A commands file is text of a few hundred kilobytes at most.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Built-in radios, then imported ones.
pub fn all(store: &Store) -> Vec<Radio> {
    let mut v = file::builtin();
    if let Some(text) = store.setting(IMPORTED).ok().flatten() {
        v.extend(file::parse(&text).radios);
    }
    v
}

/// The radio called `name`; an imported one of that name wins over a built-in.
pub fn find(store: &Store, name: &str) -> Option<Radio> {
    all(store).into_iter().rev().find(|r| r.name == name)
}

/// The names to choose from, in order.
pub fn names(store: &Store) -> Vec<String> {
    let mut seen = Vec::new();
    for r in all(store) {
        if !seen.contains(&r.name) {
            seen.push(r.name);
        }
    }
    seen
}

/// What an import did.
pub struct Imported {
    pub added: Vec<String>,
    /// "name: why" for each radio that could not be read.
    pub skipped: Vec<String>,
}

/// Take the radios in `text` (either layout) into the app. A radio imported
/// again replaces the earlier one of its name.
pub fn import(store: &mut Store, text: &str) -> Result<Imported, String> {
    if text.len() > MAX_FILE_BYTES {
        return Err("That file is too large to be a CAT commands file".into());
    }
    let loaded = file::load(text);
    if loaded.radios.is_empty() {
        return Err(match loaded.skipped.first() {
            Some((name, why)) => format!("No radio could be read. {name}: {why}"),
            None => "No radios found in that file".into(),
        });
    }
    let mut kept: Vec<Radio> =
        store.setting(IMPORTED).ok().flatten().map(|t| file::parse(&t).radios).unwrap_or_default();
    for r in &loaded.radios {
        kept.retain(|k| k.name != r.name);
    }
    kept.extend(loaded.radios.iter().cloned());
    let text: String = kept.iter().map(Radio::to_text).collect();
    store.set_setting(IMPORTED, &text).map_err(|e| e.to_string())?;
    Ok(Imported {
        added: loaded.radios.into_iter().map(|r| r.name).collect(),
        skipped: loaded.skipped.into_iter().map(|(n, w)| format!("{n}: {w}")).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const THEIRS: &str = "[Test Radio 1]\nCmdType=TEXT\nPTTOn=TX1;\nPTTOff=TX0;\nSetFreq_Data_method=TEXT\nSetFreqVfoA_Cmd=FA{};\nSetFreqVfoA_param_length=9\nReadFreqVfoA_Cmd=FA;\nReadFreqVfoA_Result_Data_method=TEXT\nReadFreqVfoA_Result_Length=12\nReadFreqVfoA_Result_Freq_Start_Pos=3\nReadFreqVfoA_Result_Freq_Length=9\n\n[Broken]\nCmdType=HEX\n";

    #[test]
    fn an_imported_file_adds_its_radios_and_names_the_ones_it_could_not_read() {
        let mut store = Store::open_in_memory().unwrap();
        let before = names(&store).len();
        let r = import(&mut store, THEIRS).unwrap();
        assert_eq!(r.added, vec!["Test Radio 1"]);
        assert_eq!(r.skipped.len(), 1);
        assert!(r.skipped[0].starts_with("Broken: "));
        assert_eq!(names(&store).len(), before + 1);
        assert!(find(&store, "Test Radio 1").is_some());
        assert!(find(&store, "Icom IC-7300").is_some(), "the built-in radios stay");
        // Imported again: replaced, not doubled.
        import(&mut store, THEIRS).unwrap();
        assert_eq!(names(&store).len(), before + 1);
    }

    #[test]
    fn a_file_with_no_readable_radio_is_refused_and_changes_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        let before = names(&store);
        assert!(import(&mut store, "[Broken]\nCmdType=HEX\n").is_err());
        assert!(import(&mut store, "nothing here").is_err());
        assert!(import(&mut store, &"x".repeat(MAX_FILE_BYTES + 1)).is_err());
        assert_eq!(names(&store), before);
    }
}
