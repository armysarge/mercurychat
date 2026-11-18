//! M3b: rig control through the service actor, against a fake rigctld and
//! the fake Mercury (or the simulator, where a session is needed).
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk, EventEnvelope,
    ModemState, RigArrangement, RigBanner, RigChange, RigConfig, RigLinkView, RigPttView, RIG_SOURCE,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::fake_rig::FakeRigctld;
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::time::Instant;

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

fn existing(net: &FakeNet) -> StationProfile {
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: net.a_base,
        data_port: net.a_base + 1,
        broadcast_port: None,
        telemetry_url: None,
    };
    p
}

fn rigctld(fake: &FakeRigctld) -> RigConfig {
    RigConfig {
        arrangement: RigArrangement::Rigctld,
        host: "127.0.0.1".into(),
        port: fake.addr.port(),
        restore_on_startup: false,
    }
}

async fn snapshot(h: &AppHandle) -> AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    }
}

/// Poll the snapshot until `pred` holds, or panic after `secs`.
async fn wait_for(h: &AppHandle, secs: u64, what: &str, pred: impl Fn(&AppSnapshot) -> bool) -> AppSnapshot {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = snapshot(h).await;
        if pred(&s) {
            return s;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}: rig = {:?}, modem = {:?}", s.rig, s.modem);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn until_state(rx: &mut Receiver<EventEnvelope>, want: ModemState, secs: u64) {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let ev = tokio::time::timeout_at(end, rx.recv()).await.expect("modem state in time").unwrap();
        if let AppEvent::ModemState { state, detail } = ev.event {
            if state == want {
                return;
            }
            assert_ne!(state, ModemState::Faulted, "faulted: {detail:?}");
        }
    }
}

/// An app on `profile` with rigctld at `rig`, its modem idle and the rig link up.
async fn ready(root: &Path, profile: StationProfile, rig: RigConfig) -> (AppHandle, Receiver<EventEnvelope>) {
    let (h, rx) = attached(root, profile, rig).await;
    wait_for(&h, 5, "the rig link", |s| s.rig.link == RigLinkView::Up && s.rig.ptt != RigPttView::Unknown).await;
    (h, rx)
}

/// As `ready`, without waiting for the rig link.
async fn attached(root: &Path, profile: StationProfile, rig: RigConfig) -> (AppHandle, Receiver<EventEnvelope>) {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(root));
    let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    until_state(&mut rx, ModemState::Listening, 15).await;
    assert_eq!(h.call(AppCommand::SetRigConfig { config: rig }).await.unwrap(), CommandOk::Done);
    (h, rx)
}

async fn set(h: &AppHandle, change: RigChange) -> Result<CommandOk, AppError> {
    h.call(AppCommand::RigSet { change }).await
}

fn count(log: &[String], line: &str) -> usize {
    log.iter().filter(|l| l.as_str() == line).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn rig_readings_reach_the_snapshot_with_source_and_age() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    rig.set_freq(7_074_000);
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    let s = wait_for(&h, 3, "a frequency", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 7_074_000)).await;
    let f = s.rig.frequency.unwrap();
    assert_eq!(f.source, RIG_SOURCE);
    assert!(f.age_ms.unwrap() < 2_000);
    assert_eq!(s.rig.mode.as_deref(), Some("PKTUSB"));
    assert_eq!(s.rig.ptt, RigPttView::Off);
    assert!(s.rig.caps.unwrap().can_set_freq);
}

#[tokio::test(flavor = "multi_thread")]
async fn frequency_set_while_idle_reaches_the_rig() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    assert_eq!(set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap(), CommandOk::Done);
    set(&h, RigChange::Mode { mode: "PKTUSB".into(), passband_hz: 2400 }).await.unwrap();
    wait_for(&h, 3, "the new frequency", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 7_105_000)).await;
    assert_eq!(rig.freq(), 7_105_000);
    wait_for(&h, 3, "the new passband", |s| s.rig.passband_hz == Some(2400)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn frequency_set_while_connected_is_refused_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    let rig = FakeRigctld::start().await;
    let (h, mut rx) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC"), rigctld(&rig)).await;
    h.call(AppCommand::Call { peer: "SIM1".into() }).await.unwrap();
    until_state(&mut rx, ModemState::Connected, 15).await;
    let err = set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap_err();
    match &err {
        AppError::Refused { reason, message } => {
            assert_eq!(*reason, "rig.refused.connected");
            assert!(message.contains("connected"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["key"], "errors.refused");
    assert_eq!(json["reason"], "rig.refused.connected");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!rig.log().iter().any(|l| l.starts_with("+F")), "{:?}", rig.log());
}

#[tokio::test(flavor = "multi_thread")]
async fn frequency_set_racing_a_call_never_lands_after_connect() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    rig.delay(Duration::from_millis(300));
    set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap();
    h.call(AppCommand::Call { peer: "ZS6XYZ".into() }).await.unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if net.log().iter().any(|l| l.contains("CONNECT ")) {
            assert!(
                rig.log().iter().any(|l| l == "+F 7105000"),
                "CONNECT went out before the frequency set reached the rig: {:?}",
                rig.log()
            );
            break;
        }
        assert!(Instant::now() < end, "no CONNECT: {:?}", net.log());
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mercury_ptt_on_with_rig_unkeyed_raises_fault() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    net.send_control(0, "PTT ON"); // the rig never keys
    let s = wait_for(&h, 3, "the PTT fault", |s| s.rig.fault == Some(RigBanner::PttNotFollowing)).await;
    assert!(s.rig.banners.contains(&RigBanner::PttNotFollowing));
    net.send_control(0, "PTT OFF");
    wait_for(&h, 3, "the fault to clear", |s| s.rig.fault.is_none()).await;
    let s = snapshot(&h).await;
    assert!(s.rig.banners.contains(&RigBanner::PttNotFollowing), "stays until dismissed");
    h.call(AppCommand::DismissRigBanner { banner: RigBanner::PttNotFollowing }).await.unwrap();
    assert!(snapshot(&h).await.rig.banners.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn rigctld_killed_while_keyed_sends_one_ptt_off() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;

    // Mercury keyed and unkeyed; the rig stuck keyed; then rigctld dies.
    rig.set_ptt(true);
    net.send_control(0, "PTT ON");
    wait_for(&h, 3, "the rig keyed", |s| s.rig.ptt == RigPttView::On).await;
    net.send_control(0, "PTT OFF");
    rig.kill();
    wait_for(&h, 5, "the critical banner", |s| s.rig.banners.contains(&RigBanner::LostWhileKeyed)).await;
    rig.restart().await;
    wait_for(&h, 20, "the PTT-off", |_| count(&rig.log(), "+T 0") == 1).await;
    wait_for(&h, 3, "the rig unkeyed", |s| s.rig.ptt == RigPttView::Off).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(count(&rig.log(), "+T 0"), 1, "exactly one");
    assert!(!rig.log().iter().any(|l| l.starts_with("+T 1")));
    h.call(AppCommand::DismissRigBanner { banner: RigBanner::LostWhileKeyed }).await.unwrap();

    // Again, but Mercury is still transmitting when the link returns.
    rig.set_ptt(true);
    net.send_control(0, "PTT ON");
    wait_for(&h, 3, "the rig keyed", |s| s.rig.ptt == RigPttView::On).await;
    rig.kill();
    wait_for(&h, 5, "the critical banner", |s| s.rig.banners.contains(&RigBanner::LostWhileKeyed)).await;
    rig.restart().await;
    wait_for(&h, 20, "the link back", |s| s.rig.link == RigLinkView::Up && s.rig.ptt == RigPttView::On).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(count(&rig.log(), "+T 0"), 1, "none while Mercury says PTT ON");
    net.send_control(0, "PTT OFF"); // the rig stays keyed
    wait_for(&h, 3, "the second PTT-off", |_| count(&rig.log(), "+T 0") == 2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rigctld_killed_while_idle_keeps_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let rig = FakeRigctld::start().await;
    let (h, mut rx) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC"), rigctld(&rig)).await;
    h.call(AppCommand::Call { peer: "SIM1".into() }).await.unwrap();
    until_state(&mut rx, ModemState::Connected, 15).await;
    rig.kill();
    wait_for(&h, 5, "the link lost", |s| matches!(s.rig.link, RigLinkView::Lost { .. })).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = snapshot(&h).await;
    assert_eq!(s.modem, ModemState::Connected, "the session lives on");
    assert!(s.session.is_some());
    assert!(!s.rig.banners.contains(&RigBanner::LostWhileKeyed), "{:?}", s.rig.banners);
}

#[tokio::test(flavor = "multi_thread")]
async fn heard_station_records_rigctld_source_when_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    rig.set_freq(14_105_000);
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    wait_for(&h, 3, "a fresh frequency", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 14_105_000)).await;
    net.air_cq(1, "ZS6XYZ", "2300");
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let CommandOk::Heard { stations } = h.call(AppCommand::Heard).await.unwrap() else { panic!() };
        if let Some(st) = stations.iter().find(|s| s.callsign == "ZS6XYZ") {
            let f = st.freq.as_ref().expect("a frequency on the heard station");
            assert_eq!((f.value, f.source.as_str()), (14_105_000, RIG_SOURCE));
            assert_eq!(st.band.as_deref(), Some("20m"));
            break;
        }
        assert!(Instant::now() < end, "ZS6XYZ was not heard");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_on_startup_is_opt_in() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    set(&h, RigChange::Frequency { hz: 10_145_000 }).await.unwrap();
    wait_for(&h, 3, "the set", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 10_145_000)).await;
    // Someone retunes the rig; reconnecting without the opt-in leaves it.
    rig.set_freq(7_000_000);
    h.call(AppCommand::SetRigConfig { config: rigctld(&rig) }).await.unwrap();
    wait_for(&h, 5, "the reading", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 7_000_000)).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(rig.freq(), 7_000_000);
    // With the opt-in, the last frequency comes back.
    h.call(AppCommand::SetRigConfig { config: RigConfig { restore_on_startup: true, ..rigctld(&rig) } }).await.unwrap();
    wait_for(&h, 5, "the restore", |_| rig.freq() == 10_145_000).await;
}

// ---- Task 6: per-band TX level and the two-owner check (telemetry WebSocket) ----

use mc_core::app::{BandLevels, OwnerCheck, TxLevelResult};
use mercury_adapter::fake_ws::FakeWs;

const STATUS: &str = r#"{"type":"status","snr":5.0,"tx_gain_db":0.0,"audio_ok":true,"busy":""}"#;

fn with_ws(net: &FakeNet, ws: &FakeWs) -> StationProfile {
    let mut p = existing(net);
    if let ModemTarget::Existing { telemetry_url, .. } = &mut p.modem {
        *telemetry_url = Some(ws.url.clone());
    }
    p
}

fn radio_list(model: &str, device: &str, method: &str) -> String {
    format!(
        r#"{{"type":"radio_list","selected":"{model}","device_path":"{device}","ptt_method":"{method}","list":[]}}"#
    )
}

async fn levels(h: &AppHandle, band: &str, tx_gain_db: f32) {
    let levels = BandLevels { tx_gain_db, tune_dbfs: -20.0 };
    assert_eq!(h.call(AppCommand::SetBandLevels { band: band.into(), levels }).await.unwrap(), CommandOk::Done);
}

#[tokio::test(flavor = "multi_thread")]
async fn band_change_applies_and_confirms_tx_level() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(STATUS));
    let rig = FakeRigctld::start().await; // starts on 20 m
    let (h, _rx) = ready(dir.path(), with_ws(&net, &ws), rigctld(&rig)).await;
    levels(&h, "40m", -3.5).await;
    set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap();
    wait_for(&h, 8, "the TX level confirmed", |s| {
        matches!(&s.rig.tx_level_applied, Some(TxLevelResult::Applied { band, db }) if band == "40m" && *db == -3.5)
    })
    .await;
    assert!(ws.received().iter().any(|t| t.contains(r#""set_tx_gain","value":"-3.50""#)), "{:?}", ws.received());
    // A band without stored levels leaves Mercury's own level alone.
    let sent = ws.received().len();
    set(&h, RigChange::Frequency { hz: 14_105_000 }).await.unwrap();
    wait_for(&h, 3, "20 m", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 14_105_000)).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(ws.received().len(), sent, "nothing stored for 20 m: nothing sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn unconfirmed_tx_level_reports_not_applied() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(STATUS));
    ws.set_answer_commands(false);
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), with_ws(&net, &ws), rigctld(&rig)).await;
    levels(&h, "40m", -3.5).await;
    set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap();
    wait_for(
        &h,
        10,
        "not applied",
        |s| matches!(&s.rig.tx_level_applied, Some(TxLevelResult::NotApplied { band, .. }) if band == "40m"),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn serial_cat_in_mercury_refuses_rig_control() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(STATUS));
    ws.set_radio_list(Some(&radio_list("1035", "/dev/ttyUSB0", "hamlib")));
    let rig = FakeRigctld::start().await;
    let (h, _rx) = attached(dir.path(), with_ws(&net, &ws), rigctld(&rig)).await;
    let s = wait_for(&h, 5, "the refusal", |s| matches!(s.rig.owner_check, OwnerCheck::Refused { .. })).await;
    let OwnerCheck::Refused { reason } = s.rig.owner_check else { unreachable!() };
    assert!(reason.contains("1035") && reason.contains("rigctld"), "{reason}");
    wait_for(&h, 3, "the link down", |s| s.rig.link != RigLinkView::Up).await;
    assert!(!rig.log().iter().any(|l| l.starts_with("+F")));
    let err = set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap_err();
    assert!(matches!(err, AppError::Refused { reason: "rig.refused.linkDown", .. }), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn different_rigctld_address_only_warns() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(STATUS));
    ws.set_radio_list(Some(&radio_list("2", "127.0.0.1:4999", "hamlib")));
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), with_ws(&net, &ws), rigctld(&rig)).await;
    wait_for(&h, 5, "the warning", |s| matches!(s.rig.owner_check, OwnerCheck::Warn { .. })).await;
    set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap();
    wait_for(&h, 3, "the set", |_| rig.freq() == 7_105_000).await;
    // Mercury pointed at our rigctld: fine.
    let ok = radio_list("2", &format!("127.0.0.1:{}", rig.addr.port()), "hamlib");
    ws.send_text(ok);
    wait_for(&h, 5, "ok", |s| s.rig.owner_check == OwnerCheck::Ok).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_radio_list_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(STATUS));
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), with_ws(&net, &ws), rigctld(&rig)).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(snapshot(&h).await.rig.owner_check, OwnerCheck::Unknown);
}

// ---- Task 7: Tune through Mercury ----

async fn tune(h: &AppHandle, on: bool) -> Result<CommandOk, AppError> {
    h.call(AppCommand::Tune { on }).await
}

/// As `ready`, with the given options.
async fn ready_opts(profile: StationProfile, rig: RigConfig, o: AppOptions) -> (AppHandle, Receiver<EventEnvelope>) {
    let h = spawn_app(Store::open_in_memory().unwrap(), o);
    let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    until_state(&mut rx, ModemState::Listening, 15).await;
    h.call(AppCommand::SetRigConfig { config: rig }).await.unwrap();
    wait_for(&h, 5, "the rig link", |s| s.rig.link == RigLinkView::Up && s.rig.ptt != RigPttView::Unknown).await;
    (h, rx)
}

/// An app whose rig follows Mercury's PTT, as a working radio does.
async fn tune_ready(dir: &Path, net: &FakeNet, rig: &FakeRigctld) -> (AppHandle, Receiver<EventEnvelope>) {
    let (h, rx) = ready(dir, existing(net), rigctld(rig)).await;
    wait_for(&h, 5, "the TUNE probe", |s| s.tune.supported.is_some()).await;
    (h, rx)
}

fn refusal(err: &AppError) -> &'static str {
    match err {
        AppError::Refused { reason, .. } => reason,
        other => panic!("not a refusal: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_probe_sets_supported_and_tune_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    assert_eq!(snapshot(&h).await.tune.supported, Some(true));
    assert!(net.log().iter().any(|l| l == "A> TUNE ?"));
    rig.set_ptt(true); // the radio keys with Mercury
    tune(&h, true).await.unwrap();
    wait_for(&h, 3, "tuning", |s| s.tune.active && s.tune.ends_utc.is_some() && net.tuning(0).is_some()).await;
    assert_eq!(net.tuning(0).as_deref(), Some("-20"), "the default tune level");
    rig.set_ptt(false);
    tune(&h, false).await.unwrap();
    wait_for(&h, 3, "stopped", |s| !s.tune.active).await;
    assert_eq!(net.tuning(0), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_to_probe_hides_tune() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    net.set_no_tune(0, true);
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    assert_eq!(snapshot(&h).await.tune.supported, Some(false));
    let err = tune(&h, true).await.unwrap_err();
    assert!(matches!(&err, AppError::Conflict { message } if message.contains("no TUNE")), "{err:?}");
    assert!(!net.log().iter().any(|l| l.starts_with("A> TUNE -")));
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_to_a_level_is_a_refusal_not_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    net.set_tune_wrong(0, true);
    tune(&h, true).await.unwrap();
    wait_for(&h, 3, "the refusal", |s| !s.tune.active).await;
    assert_eq!(snapshot(&h).await.tune.supported, Some(true), "still supported");
    let CommandOk::Activity { entries } = h.call(AppCommand::Activity).await.unwrap() else { panic!() };
    assert!(entries.iter().any(|e| e.text.contains("Tune refused")), "{entries:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_stops_after_its_limit() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let o = AppOptions { tune_limit: Some(Duration::from_millis(1000)), ..opts(dir.path()) };
    let (h, _rx) = ready_opts(existing(&net), rigctld(&rig), o).await;
    wait_for(&h, 5, "the TUNE probe", |s| s.tune.supported == Some(true)).await;
    rig.set_ptt(true);
    tune(&h, true).await.unwrap();
    wait_for(&h, 2, "tuning", |_| net.tuning(0).is_some()).await;
    rig.set_ptt(false);
    wait_for(&h, 3, "the stop", |s| !s.tune.active && net.tuning(0).is_none()).await;
    assert!(net.log().iter().any(|l| l == "A> TUNE OFF"));
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_refused_when_ptt_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    rig.ptt_error(Some(-11));
    wait_for(&h, 3, "PTT unknown", |s| s.rig.ptt == RigPttView::Unknown).await;
    assert_eq!(refusal(&tune(&h, true).await.unwrap_err()), "rig.refused.pttUnknown");
    // Idle tuning of the frequency is still allowed (owner, 18 Nov).
    set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_refused_while_connected() {
    let dir = tempfile::tempdir().unwrap();
    let rig = FakeRigctld::start().await;
    let (h, mut rx) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC"), rigctld(&rig)).await;
    h.call(AppCommand::Call { peer: "SIM1".into() }).await.unwrap();
    until_state(&mut rx, ModemState::Connected, 15).await;
    assert_eq!(refusal(&tune(&h, true).await.unwrap_err()), "rig.refused.connected");
}

#[tokio::test(flavor = "multi_thread")]
async fn call_and_cq_are_refused_while_tuning() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    rig.set_ptt(true);
    tune(&h, true).await.unwrap();
    let err = h.call(AppCommand::Call { peer: "ZS6XYZ".into() }).await.unwrap_err();
    assert_eq!(refusal(&err), "rig.refused.tuning");
    assert!(h.call(AppCommand::SendCq).await.is_err(), "no CQ while tuning");
    assert_eq!(refusal(&set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap_err()), "rig.refused.tuning");
    tune(&h, false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_check_runs_during_tune() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    tune(&h, true).await.unwrap(); // Mercury keys; the rig does not
    wait_for(&h, 3, "the PTT fault", |s| s.rig.fault == Some(RigBanner::PttNotFollowing)).await;
    tune(&h, false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_level_follows_the_band_and_can_change_live() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    let levels = BandLevels { tx_gain_db: 0.0, tune_dbfs: -30.0 };
    h.call(AppCommand::SetBandLevels { band: "20m".into(), levels }).await.unwrap();
    rig.set_ptt(true);
    tune(&h, true).await.unwrap();
    wait_for(&h, 3, "tuning at -30", |_| net.tuning(0).as_deref() == Some("-30")).await;
    h.call(AppCommand::SetTuneLevel { dbfs: -25.0 }).await.unwrap();
    wait_for(&h, 3, "tuning at -25", |_| net.tuning(0).as_deref() == Some("-25")).await;
    assert!(h.call(AppCommand::SetTuneLevel { dbfs: 3.0 }).await.is_err(), "out of range");
    rig.set_ptt(false);
    tune(&h, false).await.unwrap();
    wait_for(&h, 3, "stopped", |s| !s.tune.active).await;
    assert!(h.call(AppCommand::SetTuneLevel { dbfs: -25.0 }).await.is_err(), "only while tuning");
}
