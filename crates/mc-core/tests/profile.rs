//! SET-01 / SET-03 / RF-02: station profiles validate, normalise and persist.

use mc_core::profile::{from_row, to_row, BuiltInPorts, FieldError, ModemTarget, StationProfile};
use mc_storage::Store;

fn fields(errs: &[FieldError]) -> Vec<&str> {
    errs.iter().map(|e| e.field.as_str()).collect()
}

fn existing(p: &mut StationProfile) -> (&mut u16, &mut u16, &mut Option<u16>, &mut Option<String>) {
    match &mut p.modem {
        ModemTarget::Existing { control_port, data_port, broadcast_port, telemetry_url, .. } => {
            (control_port, data_port, broadcast_port, telemetry_url)
        }
        _ => panic!("not an existing-modem profile"),
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
    // M3a Task 7: exactly ws://host:port/path, at most 256 characters.
    let shape = "Use ws://host:port/path, e.g. ws://127.0.0.1:10000/websocket";
    for bad in [
        "http://127.0.0.1:10000/websocket",
        "ws://127.0.0.1:10000",
        "ws://127.0.0.1/websocket",
        "ws://:10000/websocket",
        "ws://127.0.0.1:0/websocket",
        "ws://127.0.0.1:65536/websocket",
        "ws://127.0.0.1:10x00/websocket",
        "ws://user:pw@127.0.0.1:10000/websocket",
        "ws://127.0.0.1:10000/web socket",
        "ws://127.0.0.1:10000/websocket\u{7}",
        "ws://bad host:10000/websocket",
        "ws://[::1:10000/websocket",
    ] {
        *existing(&mut p).3 = Some(bad.into());
        let errs = p.validate().unwrap_err();
        assert_eq!(fields(&errs), ["telemetryUrl"], "{bad:?}");
        assert_eq!(errs[0].message, shape, "{bad:?}");
    }
    *existing(&mut p).3 = Some("wss://127.0.0.1:10000/websocket".into());
    let errs = p.validate().unwrap_err();
    assert_eq!(errs[0].message, "wss:// (TLS) is not supported: use ws://host:port/path");
    let long = format!("ws://127.0.0.1:10000/{}", "a".repeat(240));
    *existing(&mut p).3 = Some(long);
    let errs = p.validate().unwrap_err();
    assert_eq!((errs[0].field.as_str(), errs[0].message.as_str()), ("telemetryUrl", "At most 256 characters"));

    for good in [
        "ws://127.0.0.1:10000/websocket",
        " ws://mercury.local:10000/websocket ",
        "ws://[::1]:10000/websocket",
        "ws://pi:1/",
    ] {
        *existing(&mut p).3 = Some(good.into());
        let v = p.validate().unwrap_or_else(|e| panic!("{good:?}: {e:?}"));
        assert_eq!(existing(&mut v.clone()).3.as_deref(), Some(good.trim()));
    }
    // No telemetry is fine: it is optional (shown as unconfigured).
    *existing(&mut p).3 = Some("  ".into());
    assert_eq!(existing(&mut p.validate().unwrap()).3, &None);
    *existing(&mut p).3 = None;
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

#[test]
fn station_info_round_trips_through_the_row() {
    let mut p = StationProfile::new_simulator("ZS6ABC");
    p.qth = Some("Pretoria".into());
    p.rig = Some("IC-7300".into());
    p.power = Some("50 W".into());
    p.antenna = Some("EFHW".into());
    p.email = Some("me@example.org".into());
    p.about = Some("Digital modes".into());
    let mut store = Store::open_in_memory().unwrap();
    store.save_profile(&to_row(&p)).unwrap();
    let back = from_row(&store.profiles().unwrap()[0]).unwrap();
    assert_eq!(back, p);
    // Saving again with the fields cleared clears them (no stale values).
    p.rig = None;
    store.save_profile(&to_row(&p)).unwrap();
    assert_eq!(from_row(&store.profiles().unwrap()[0]).unwrap().rig, None);
}

/// M5 §5.1: a built-in profile keeps its four ports through the store.
#[test]
fn builtin_profile_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("app.db")).unwrap();
    let mut p = StationProfile::new_builtin("ZS6ABC");
    p.modem = ModemTarget::BuiltIn { ports: BuiltInPorts { control: 8400, broadcast: 8200, ui: 10100, rigctld: 4533 } };
    let p = p.validate().unwrap();
    store.save_profile(&to_row(&p)).unwrap();
    let back = from_row(&store.profiles().unwrap()[0]).unwrap();
    assert_eq!(back, p);
    assert_eq!(StationProfile::new_builtin("ZS6ABC").modem, ModemTarget::BuiltIn { ports: BuiltInPorts::default() });
    assert_eq!(BuiltInPorts::default(), BuiltInPorts { control: 8300, broadcast: 8100, ui: 10000, rigctld: 4532 });
}

/// M5 §5.1: the five ports (data is control + 1) must all differ, and none is 0.
#[test]
fn builtin_ports_must_differ() {
    let with = |ports: BuiltInPorts| {
        let mut p = StationProfile::new_builtin("ZS6ABC");
        p.modem = ModemTarget::BuiltIn { ports };
        p.validate()
    };
    let d = BuiltInPorts::default();
    assert!(with(d).is_ok());
    let errs = with(BuiltInPorts { broadcast: 8301, ..d }).unwrap_err();
    assert_eq!(fields(&errs), ["broadcastPort"], "broadcast on the data port");
    let errs = with(BuiltInPorts { ui: 8300, ..d }).unwrap_err();
    assert_eq!(fields(&errs), ["uiPort"]);
    let errs = with(BuiltInPorts { rigctld: 0, ..d }).unwrap_err();
    assert_eq!(fields(&errs), ["rigctldPort"]);
    let errs = with(BuiltInPorts { control: 65535, ..d }).unwrap_err();
    assert_eq!(fields(&errs), ["controlPort"], "no room for the data port");
}

#[test]
fn existing_and_simulator_profiles_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("app.db")).unwrap();
    for p in [StationProfile::new_existing("ZS6ABC"), StationProfile::new_simulator("ZS6SIM")] {
        let p = p.validate().unwrap();
        store.save_profile(&to_row(&p)).unwrap();
        let back = store.profiles().unwrap().into_iter().find(|r| r.id == p.id).unwrap();
        assert_eq!(from_row(&back).unwrap(), p);
        assert_eq!((back.ui_port, back.rigctld_port), (None, None));
    }
}

/// M5 §5.6: a release build has no simulator. A stored simulator profile is
/// unavailable, said plainly, never a fault. Run by scripts/check.sh with
/// mc-core built without `sim`.
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn simulator_profile_without_sim_is_unavailable() {
    use mc_core::app::{spawn_app, AppCommand, AppOptions, CommandOk, ModemState};
    let dir = tempfile::tempdir().unwrap();
    let o = AppOptions { sim_root: Some(dir.path().to_path_buf()), ..AppOptions::default() };
    let h = spawn_app(Store::open_in_memory().unwrap(), o);
    let id = match h.call(AppCommand::SaveProfile { profile: StationProfile::new_simulator("ZS6ABC") }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    let s = match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(s.modem, ModemState::Unavailable);
    assert_eq!(s.modem_detail.as_deref(), Some("The simulator is not available in this build."));
}
