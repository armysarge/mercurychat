//! Rig settings, per-band levels and presets (M3b Task 3).

use mc_core::app::{BandLevels, RigArrangement, RigConfig};
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
    let mut store = Store::open(&dir.path().join("a.db")).unwrap();
    let d = RigConfig::default();
    assert_eq!(d.arrangement, RigArrangement::Mercury, "the M3a behaviour: Mercury's read-only frequency");
    assert_eq!((d.host.as_str(), d.port), ("127.0.0.1", 4532));
    assert!(!d.restore_on_startup, "restore is opt-in (RIG-03)");
    assert_eq!(RigConfig::load(&store, "p1"), d);
    let c = RigConfig {
        arrangement: RigArrangement::Rigctld,
        host: "10.0.0.2".into(),
        port: 4632,
        restore_on_startup: true,
    };
    c.save(&mut store, "p1").unwrap();
    assert_eq!(RigConfig::load(&store, "p1"), c);
    assert_eq!(RigConfig::load(&store, "p2"), d, "per profile");
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
