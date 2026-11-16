//! SET-01 / SET-03 / RF-02: station profiles validate, normalise and persist.

use mc_core::profile::{from_row, to_row, FieldError, ModemTarget, StationProfile};
use mc_storage::Store;

fn fields(errs: &[FieldError]) -> Vec<&str> {
    errs.iter().map(|e| e.field.as_str()).collect()
}

fn existing(p: &mut StationProfile) -> (&mut u16, &mut u16, &mut Option<u16>, &mut Option<String>) {
    match &mut p.modem {
        ModemTarget::Existing { control_port, data_port, broadcast_port, telemetry_url, .. } => {
            (control_port, data_port, broadcast_port, telemetry_url)
        }
        ModemTarget::Simulator => panic!("not an existing-modem profile"),
    }
}

#[test]
fn callsign_is_normalised() {
    let p = StationProfile::new_existing(" zs6abc/p ").validate().unwrap();
    assert_eq!(p.callsign, "ZS6ABC/P");
}

#[test]
fn bad_callsigns_are_refused() {
    for bad in ["", "ZS 6ABC", &"A".repeat(16), "ZS6ABC\u{200B}", "ZS6ÄBC"] {
        let errs = StationProfile::new_existing(bad).validate().unwrap_err();
        assert_eq!(fields(&errs), ["callsign"], "{bad:?}");
        assert_eq!(errs[0].message, "Use 1–15 letters, digits, / or -");
    }
}

#[test]
fn grid_is_normalised_and_checked() {
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.grid = Some("kg33xi".into());
    assert_eq!(p.validate().unwrap().grid.as_deref(), Some("KG33xi"));
    p.grid = Some("KG3".into());
    let errs = p.validate().unwrap_err();
    assert_eq!(fields(&errs), ["grid"]);
    assert_eq!(errs[0].message, "Use a 4, 6 or 8 character Maidenhead locator");
    p.grid = Some("  ".into());
    assert_eq!(p.validate().unwrap().grid, None, "blank grid means none");
}

#[test]
fn name_is_trimmed_and_bounded() {
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.name = Some("  Shaun  ".into());
    assert_eq!(p.validate().unwrap().name.as_deref(), Some("Shaun"));
    p.name = Some("x".repeat(65));
    assert_eq!(fields(&p.validate().unwrap_err()), ["name"]);
}

#[test]
fn ports_must_be_distinct() {
    let mut p = StationProfile::new_existing("ZS6ABC");
    *existing(&mut p).1 = 8300;
    let errs = p.validate().unwrap_err();
    assert_eq!(fields(&errs), ["dataPort"]);
    assert_eq!(errs[0].message, "Must differ from the control port");

    let mut p = StationProfile::new_existing("ZS6ABC");
    *existing(&mut p).2 = Some(8301);
    let errs = p.validate().unwrap_err();
    assert_eq!(fields(&errs), ["broadcastPort"]);
    assert_eq!(errs[0].message, "Must differ from the control and data ports");
}

#[test]
fn zero_port_is_refused() {
    let mut p = StationProfile::new_existing("ZS6ABC");
    *existing(&mut p).0 = 0;
    let errs = p.validate().unwrap_err();
    assert_eq!(fields(&errs), ["controlPort"]);
    assert_eq!(errs[0].message, "Port must be 1–65535");
}

#[test]
fn host_must_be_present() {
    for bad in ["", "two words", &"h".repeat(254)] {
        let mut p = StationProfile::new_existing("ZS6ABC");
        if let ModemTarget::Existing { host, .. } = &mut p.modem {
            *host = bad.to_string();
        }
        let errs = p.validate().unwrap_err();
        assert_eq!(fields(&errs), ["host"], "{bad:?}");
        assert_eq!(errs[0].message, "Enter a host name or address");
    }
}

#[test]
fn telemetry_url_must_be_websocket() {
    let mut p = StationProfile::new_existing("ZS6ABC");
    *existing(&mut p).3 = Some("http://127.0.0.1:10000".into());
    let errs = p.validate().unwrap_err();
    assert_eq!(fields(&errs), ["telemetryUrl"]);
    assert_eq!(errs[0].message, "Use a ws:// or wss:// address");
    *existing(&mut p).3 = Some("ws://127.0.0.1:10000".into());
    p.validate().unwrap();
}

#[test]
fn errors_are_collected() {
    let mut p = StationProfile::new_existing("");
    *existing(&mut p).0 = 0;
    assert_eq!(fields(&p.validate().unwrap_err()), ["callsign", "controlPort"]);
}

#[test]
fn simulator_needs_no_endpoint() {
    let p = StationProfile::new_simulator("zs6abc").validate().unwrap();
    assert_eq!(p.modem, ModemTarget::Simulator);
    assert_eq!(p.callsign, "ZS6ABC");
}

#[test]
fn profiles_answer_calls_by_default() {
    assert!(!StationProfile::new_existing("ZS6ABC").receive_only);
    assert!(!StationProfile::new_simulator("ZS6ABC").receive_only);
}

#[test]
fn new_existing_uses_mercury_defaults() {
    let p = StationProfile::new_existing("ZS6ABC");
    assert_eq!(
        p.modem,
        ModemTarget::Existing {
            host: "127.0.0.1".into(),
            control_port: 8300,
            data_port: 8301,
            broadcast_port: Some(8100),
            telemetry_url: None,
        }
    );
    assert!(!p.lab);
    assert_eq!(p.id.len(), 36, "uuid");
}

#[test]
fn profile_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.name = Some("Home".into());
    p.grid = Some("KG33xi".into());
    p.lab = true;
    p.receive_only = true;
    let p = p.validate().unwrap();
    let sim = StationProfile::new_simulator("ZS6ABC").validate().unwrap();
    {
        let mut s = Store::open(&path).unwrap();
        s.save_profile(&to_row(&p)).unwrap();
        s.save_profile(&to_row(&sim)).unwrap();
    }
    let s = Store::open(&path).unwrap();
    let back: Vec<StationProfile> = s.profiles().unwrap().iter().map(|r| from_row(r).unwrap()).collect();
    assert_eq!(back, vec![p, sim]);
}

#[test]
fn save_updates_in_place_and_delete_removes() {
    let mut s = Store::open_in_memory().unwrap();
    let mut p = StationProfile::new_existing("ZS6ABC").validate().unwrap();
    s.save_profile(&to_row(&p)).unwrap();
    p.name = Some("Renamed".into());
    s.save_profile(&to_row(&p)).unwrap();
    let rows = s.profiles().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(from_row(&rows[0]).unwrap().name.as_deref(), Some("Renamed"));
    assert!(s.delete_profile(&p.id).unwrap());
    assert!(!s.delete_profile(&p.id).unwrap());
    assert!(s.profiles().unwrap().is_empty());
}

#[test]
fn unknown_modem_kind_is_an_error() {
    let mut row = to_row(&StationProfile::new_simulator("ZS6ABC"));
    row.modem_kind = "teleport".into();
    assert_eq!(from_row(&row).unwrap_err().field, "modemKind");
}

#[test]
fn settings_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    {
        let mut s = Store::open(&path).unwrap();
        assert_eq!(s.setting("activeProfileId").unwrap(), None);
        s.set_setting("activeProfileId", "abc").unwrap();
        s.set_setting("activeProfileId", "def").unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.setting("activeProfileId").unwrap().as_deref(), Some("def"));
}

#[test]
fn a_new_profile_from_the_ui_gets_an_id() {
    // The UI drafts a profile with no id; saving must not store it under "".
    let mut draft = StationProfile::new_simulator("ZS6ABC");
    draft.id = " ".into();
    let p = draft.validate().unwrap();
    assert!(!p.id.trim().is_empty());
    assert_ne!(p.id, StationProfile::new_simulator("ZS6ABC").validate().unwrap().id);
}
