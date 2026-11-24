//! Rig settings, per-band levels and presets (M3b Task 3).

use mc_core::app::{
    BandLevels, CatLink, Endpoint, FreqControl, LineState, Parity, PttLine, PttMethod, PttSettings, RigConfig,
};
use mc_core::band::band_for;
use mc_core::presets::presets;
use mc_storage::Store;
use std::collections::HashSet;

#[test]
fn every_preset_is_inside_its_band() {
    for region in [1u8, 2, 3] {
        let ps = presets(region);
        let mut keys = HashSet::new();
        let mut bands = HashSet::new();
        for p in &ps {
            let band =
                band_for(p.freq_hz).unwrap_or_else(|| panic!("{} at {} Hz is outside every band", p.key, p.freq_hz));
            bands.insert(band);
            assert_eq!(p.mode, "PKTUSB", "{}", p.key);
            assert!(keys.insert(p.key), "{} twice", p.key);
            assert!(p.name.contains("suggested"), "{} must say it is a suggestion", p.name);
            assert!(!p.purpose.is_empty(), "{}", p.key);
        }
        for b in ["80m", "40m", "30m", "20m", "17m", "15m", "12m", "10m"] {
            assert!(bands.contains(b), "region {region} has nothing on {b}");
        }
    }
    assert!(presets(4).is_empty());
}

#[test]
fn rig_config_defaults_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("a.db")).unwrap();
    let d = RigConfig::default();
    assert_eq!(d.freq_control, FreqControl::None, "the M3a behaviour: Mercury's read-only frequency");
    assert_eq!(d.ptt.method, PttMethod::LeaveMercury);
    assert_eq!((d.external.host.as_str(), d.external.port), ("127.0.0.1", 4532));
    assert_eq!((d.flrig.host.as_str(), d.flrig.port), ("127.0.0.1", 12345));
    assert_eq!((d.cat.baud, d.cat.data_bits, d.cat.stop_bits, d.cat.wait_ms), (9600, 8, 1, 0));
    assert_eq!((d.read_every_s, d.offset_hz, d.ptt.cm108_gpio), (2, 0, 3));
    assert_eq!(d.data_mode, "PKTUSB");
    assert!(!d.restore_on_startup, "restore is opt-in (RIG-03)");
    assert!(d.validate().is_empty(), "{:?}", d.validate());
    assert_eq!(RigConfig::load(&store, "p1"), d);
    assert_eq!(RigConfig::load(&store, "p2"), d, "per profile");
}

#[test]
fn v2_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("a.db")).unwrap();
    let mut c = RigConfig { freq_control: FreqControl::ManagedRigctld, ..RigConfig::default() };
    c.cat.model = Some(3073);
    c.cat.serial_port = "COM10".into();
    c.cat.parity = Parity::Even;
    c.cat.dtr = LineState::On;
    c.ptt = PttSettings {
        method: PttMethod::SerialLine,
        line: PttLine::Dtr,
        invert: true,
        ptt_port: "COM11".into(),
        cm108_gpio: 2,
    };
    c.offset_hz = -1500;
    c.read_every_s = 5;
    c.hamlib_dir = r"C:\Hamlib\bin".into();
    c.save(&mut store, "p1").unwrap();
    assert_eq!(RigConfig::load(&store, "p1"), c);
}

fn legacy(store: &mut Store, json: &str) -> RigConfig {
    store.set_setting("profile.p1.rig.config", json).unwrap();
    RigConfig::load(store, "p1")
}

#[test]
fn legacy_rigctld_becomes_external() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("a.db")).unwrap();
    let c = legacy(&mut store, r#"{"arrangement":"rigctld","host":"10.0.0.5","port":4600,"restoreOnStartup":true}"#);
    assert_eq!(c.freq_control, FreqControl::ExternalRigctld);
    assert_eq!(c.external, Endpoint { host: "10.0.0.5".into(), port: 4600 });
    assert_eq!(c.ptt.method, PttMethod::Cat);
    assert!(c.restore_on_startup);
}

#[test]
fn legacy_mercury_and_none_become_none_leave_mercury() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("a.db")).unwrap();
    for a in ["mercury", "none"] {
        let c = legacy(
            &mut store,
            &format!(r#"{{"arrangement":"{a}","host":"127.0.0.1","port":4532,"restoreOnStartup":false}}"#),
        );
        assert_eq!(c.freq_control, FreqControl::None, "{a}");
        assert_eq!(c.ptt.method, PttMethod::LeaveMercury, "{a}");
    }
}

#[test]
fn validate_rejects() {
    type Bend = fn(&mut RigConfig);
    let cases: [(Bend, &str); 8] = [
        (|c| c.read_every_s = 3, "readEverySeconds"),
        (|c| c.offset_hz = 10_000_001, "offsetHz"),
        (|c| c.cat.data_bits = 6, "dataBits"),
        (|c| c.cat.stop_bits = 3, "stopBits"),
        (|c| c.cat.wait_ms = 1001, "waitMs"),
        (|c| c.ptt.cm108_gpio = 5, "cm108Gpio"),
        (
            |c| {
                c.freq_control = FreqControl::ManagedRigctld;
                c.cat.serial_port = "COM3".into();
            },
            "model",
        ),
        (
            |c| {
                c.freq_control = FreqControl::ManagedRigctld;
                c.cat.model = Some(1);
                c.cat.link = CatLink::Serial;
                c.cat.serial_port = String::new();
            },
            "serialPort",
        ),
    ];
    for (bend, field) in cases {
        let mut c = RigConfig::default();
        bend(&mut c);
        let errs = c.validate();
        assert_eq!(errs.len(), 1, "{field}: {errs:?}");
        assert_eq!(errs[0].field, field);
    }
}

#[test]
fn band_levels_reject_out_of_range() {
    assert_eq!(BandLevels::default(), BandLevels { tx_gain_db: 0.0, tune_dbfs: -20.0 });
    for ok in [BandLevels { tx_gain_db: -20.0, tune_dbfs: -60.0 }, BandLevels { tx_gain_db: 20.0, tune_dbfs: 0.0 }] {
        assert!(ok.validate().is_ok(), "{ok:?}");
    }
    for bad in [
        BandLevels { tx_gain_db: 20.5, tune_dbfs: -20.0 },
        BandLevels { tx_gain_db: -20.5, tune_dbfs: -20.0 },
        BandLevels { tx_gain_db: 0.0, tune_dbfs: 0.5 },
        BandLevels { tx_gain_db: 0.0, tune_dbfs: -61.0 },
        BandLevels { tx_gain_db: f32::NAN, tune_dbfs: -20.0 },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("a.db")).unwrap();
    assert!(BandLevels::load_all(&store, "p1").is_empty());
    let l = BandLevels { tx_gain_db: -3.5, tune_dbfs: -25.0 };
    BandLevels::save(&mut store, "p1", "40m", l).unwrap();
    assert_eq!(BandLevels::load_all(&store, "p1").get("40m"), Some(&l));
    assert_eq!(BandLevels::for_band(&store, "p1", "20m"), BandLevels::default());
}

#[test]
fn rigctld_spec_maps_settings() {
    use mc_core::app::rigctld_spec_for;
    let listen = "127.0.0.1:4532".parse().unwrap();
    let mut c = RigConfig { freq_control: FreqControl::ManagedRigctld, ..RigConfig::default() };
    c.cat.model = Some(3073);
    c.cat.serial_port = "COM10".into();
    c.cat.wait_ms = 100;
    c.cat.dtr = LineState::Off;
    c.ptt.method = PttMethod::SerialLine;
    c.ptt.ptt_port = "COM11".into();
    let s = rigctld_spec_for(&c, listen).unwrap();
    assert_eq!((s.model, s.rig_path.as_str(), s.baud), (3073, "COM10", Some(9600)));
    let conf: Vec<String> = s.conf.iter().map(|(k, v)| format!("{k}={v}")).collect();
    assert_eq!(conf, ["serial_parity=None", "data_bits=8", "stop_bits=1", "post_write_delay=100", "dtr_state=OFF"]);
    assert_eq!((s.ptt_type, s.ptt_path.as_deref()), ("RTS", Some("COM11")));

    c.ptt.line = PttLine::Dtr;
    c.ptt.ptt_port = String::new();
    assert_eq!(rigctld_spec_for(&c, listen).unwrap().ptt_type, "DTR");
    assert_eq!(rigctld_spec_for(&c, listen).unwrap().ptt_path, None);
    for (m, want) in [
        (PttMethod::Cat, "RIG"),
        // CM108 is Mercury's own (spec §5.2): rigctld reads PTT from the rig.
        (PttMethod::Cm108, "RIG"),
        (PttMethod::NoPtt, "NONE"),
        (PttMethod::LeaveMercury, "RIG"),
    ] {
        c.ptt.method = m;
        assert_eq!(rigctld_spec_for(&c, listen).unwrap().ptt_type, want, "{m:?}");
    }

    c.cat.link = CatLink::Tcp;
    c.cat.tcp_addr = "192.168.1.20:50001".into();
    let s = rigctld_spec_for(&c, listen).unwrap();
    assert_eq!((s.rig_path.as_str(), s.baud, s.conf.len()), ("192.168.1.20:50001", None, 0));

    c.cat.model = None;
    assert!(rigctld_spec_for(&c, listen).is_none(), "no model: nothing to start");
}

#[test]
fn mercury_ptt_mapping_table() {
    use mc_core::app::mercury_ptt_for;
    use mercury_adapter::telemetry::TelemetryCmd;
    let managed: std::net::SocketAddr = "127.0.0.1:4532".parse().unwrap();
    let cmd = |m: &str, d: &str, model: i32, line: &str, invert: &str, gpio: u8| TelemetryCmd::SetPttConfig {
        method: m.into(),
        device: d.into(),
        model,
        speed: 0,
        line: line.into(),
        invert: invert.into(),
        gpio,
    };
    let with = |fc: FreqControl, m: PttMethod| {
        let mut c = RigConfig { freq_control: fc, ..RigConfig::default() };
        c.ptt.method = m;
        c.cat.serial_port = "COM10".into();
        c.external = Endpoint { host: "10.0.0.5".into(), port: 4600 };
        c
    };
    for fc in [FreqControl::None, FreqControl::ManagedRigctld, FreqControl::ExternalRigctld, FreqControl::Flrig] {
        assert_eq!(mercury_ptt_for(&with(fc, PttMethod::LeaveMercury), managed), None, "{fc:?}");
        assert_eq!(mercury_ptt_for(&with(fc, PttMethod::NoPtt), managed), Some(cmd("none", "", 0, "rts", "none", 3)));
    }
    for m in [PttMethod::Cat, PttMethod::SerialLine] {
        assert_eq!(
            mercury_ptt_for(&with(FreqControl::ManagedRigctld, m), managed),
            Some(cmd("hamlib", "127.0.0.1:4532", 2, "rts", "none", 3)),
            "{m:?}"
        );
        assert_eq!(
            mercury_ptt_for(&with(FreqControl::ExternalRigctld, m), managed),
            Some(cmd("hamlib", "10.0.0.5:4600", 2, "rts", "none", 3)),
            "{m:?}"
        );
    }
    let mut serial = with(FreqControl::None, PttMethod::SerialLine);
    serial.ptt.line = PttLine::Dtr;
    serial.ptt.invert = true;
    assert_eq!(mercury_ptt_for(&serial, managed), Some(cmd("serial", "COM10", 0, "dtr", "dtr", 3)));
    serial.ptt.ptt_port = "COM11".into();
    serial.freq_control = FreqControl::Flrig;
    assert_eq!(mercury_ptt_for(&serial, managed), Some(cmd("serial", "COM11", 0, "dtr", "dtr", 3)));
    // CM108 is a HID device, not the COM port: Mercury drives it with the
    // chosen GPIO whatever runs the CAT (spec §5.2; final review).
    for fc in [FreqControl::None, FreqControl::ManagedRigctld, FreqControl::ExternalRigctld, FreqControl::Flrig] {
        let mut cm = with(fc, PttMethod::Cm108);
        cm.ptt.cm108_gpio = 2;
        assert_eq!(mercury_ptt_for(&cm, managed), Some(cmd("cm108", "", 0, "rts", "none", 2)), "{fc:?}");
    }
}

#[test]
fn cat_ptt_needs_a_rigctld() {
    for fc in [FreqControl::None, FreqControl::Flrig] {
        let mut c = RigConfig { freq_control: fc, ..RigConfig::default() };
        c.ptt.method = PttMethod::Cat;
        let errs = c.validate();
        assert_eq!(errs.len(), 1, "{fc:?}: {errs:?}");
        assert_eq!(errs[0].field, "pttMethod");
    }
}

/// The built-in CAT engine (M5 Task 11e): it needs a radio and a port, and
/// Mercury keys through it exactly as through MercuryChat's own rigctld.
#[test]
fn the_cat_engine_needs_a_radio_and_a_port_and_keys_like_the_managed_rigctld() {
    use mc_core::app::mercury_ptt_for;
    let managed: std::net::SocketAddr = "127.0.0.1:4532".parse().unwrap();
    let mut c = RigConfig { freq_control: FreqControl::CatFile, ..RigConfig::default() };
    let fields: Vec<_> = c.validate().into_iter().map(|e| e.field).collect();
    assert_eq!(fields, vec!["radio", "serialPort"]);
    c.cat.radio = "Icom IC-7300".into();
    c.cat.serial_port = "COM3".into();
    assert!(c.validate().is_empty());
    c.ptt.method = PttMethod::Cat;
    assert!(c.validate().is_empty(), "CAT PTT is allowed through the engine");
    assert_eq!(
        mercury_ptt_for(&c, managed),
        mercury_ptt_for(&RigConfig { freq_control: FreqControl::ManagedRigctld, ..c.clone() }, managed),
    );
}
