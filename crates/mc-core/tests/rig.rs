//! M3b: rig control through the service actor, against a fake rigctld and
//! the fake Mercury (or the simulator, where a session is needed).
#![cfg(feature = "sim")]

use chrono::NaiveTime;
use mc_core::app::{
    spawn_app, ActivityEntry, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk,
    EventEnvelope, FrequencyEntry, ModemState, RigBanner, RigChange, RigConfig, RigLinkView, RigPttView, RigTestStep,
    Schedule, ScheduleSlot, RIG_SOURCE,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::fake_rig::FakeRigctld;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
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
    RigConfig::external("127.0.0.1", fake.addr.port())
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

/// Wait for the first `CONNECT` Mercury is sent; its time, or None by `secs`.
async fn connect_sent(net: &FakeNet, secs: u64) -> Option<Instant> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if net.log().iter().any(|l| l.contains("CONNECT ")) {
            return Some(Instant::now());
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn a_frequency_set_the_rig_never_confirms_fails_the_held_call() {
    // Final review #2: a set that times out may still land later; calling
    // anyway would QSY the rig mid-call. The call fails instead.
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    rig.delay(Duration::from_millis(1500));
    set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap();
    h.call(AppCommand::Call { peer: "ZS6XYZ".into() }).await.unwrap();
    assert_eq!(connect_sent(&net, 3).await, None, "called with the frequency change unconfirmed: {:?}", net.log());
    wait_for(&h, 3, "the call given up", |s| s.modem == ModemState::Listening).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_list_pick_holds_a_call_until_the_mode_is_set_too() {
    // Final review #3: a list pick sends F then M; the call waits for both.
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    let entry = match h.call(AppCommand::Frequencies).await.unwrap() {
        CommandOk::Frequencies { entries } => entries.into_iter().next().expect("a suggested entry"),
        other => panic!("{other:?}"),
    };
    rig.delay(Duration::from_millis(400));
    let t0 = Instant::now();
    h.call(AppCommand::TuneToEntry { id: entry.id.clone() }).await.unwrap();
    h.call(AppCommand::Call { peer: "ZS6XYZ".into() }).await.unwrap();
    let at = connect_sent(&net, 5).await.expect("a CONNECT");
    let waited = at.saturating_duration_since(t0);
    assert!(
        waited >= Duration::from_millis(700),
        "CONNECT after {waited:?}: before the mode set (2 x 400 ms) finished"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn saving_rig_settings_while_the_rig_may_be_keyed_keeps_the_safety_state() {
    // Final review #5: saving the rig settings restarts the link; it must not
    // forget the critical banner or the one recovery PTT-off. (Save stays
    // allowed: the operator may need it to point at a moved rigctld.)
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    rig.set_ptt(true);
    net.send_control(0, "PTT ON");
    wait_for(&h, 3, "the rig keyed", |s| s.rig.ptt == RigPttView::On).await;
    net.send_control(0, "PTT OFF");
    rig.kill();
    wait_for(&h, 5, "the critical banner", |s| s.rig.banners.contains(&RigBanner::LostWhileKeyed)).await;
    h.call(AppCommand::SetRigConfig { config: rigctld(&rig) }).await.unwrap();
    assert!(snapshot(&h).await.rig.banners.contains(&RigBanner::LostWhileKeyed), "the banner stays");
    rig.restart().await;
    wait_for(&h, 20, "the recovery PTT-off", |_| count(&rig.log(), "+T 0") == 1).await;
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
async fn tune_stops_when_the_rig_link_is_lost() {
    // Final review #6: no carrier without the cross-check.
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    rig.set_ptt(true);
    tune(&h, true).await.unwrap();
    wait_for(&h, 3, "tuning", |s| s.tune.active && net.tuning(0).is_some()).await;
    rig.kill();
    wait_for(&h, 5, "Tune stopped", |s| !s.tune.active && net.tuning(0).is_none()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_stops_on_a_ptt_fault() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = tune_ready(dir.path(), &net, &rig).await;
    tune(&h, true).await.unwrap(); // Mercury keys; the rig does not
    wait_for(&h, 3, "the PTT fault", |s| s.rig.fault == Some(RigBanner::PttNotFollowing)).await;
    wait_for(&h, 3, "Tune stopped", |s| !s.tune.active && net.tuning(0).is_none()).await;
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

// ---- Task 8: frequency list, region presets and the QSY schedule ----

fn entry(name: &str, hz: u64) -> FrequencyEntry {
    FrequencyEntry { name: name.into(), freq_hz: hz, mode: "PKTUSB".into(), ..FrequencyEntry::default() }
}

async fn frequencies(h: &AppHandle) -> Vec<FrequencyEntry> {
    match h.call(AppCommand::Frequencies).await.unwrap() {
        CommandOk::Frequencies { entries } => entries,
        other => panic!("{other:?}"),
    }
}

/// Save `e` and return it as stored (with its id).
async fn save_entry(h: &AppHandle, e: FrequencyEntry) -> FrequencyEntry {
    let name = e.name.clone();
    match h.call(AppCommand::SaveFrequency { entry: e }).await.unwrap() {
        CommandOk::Frequencies { entries } => entries.into_iter().find(|x| x.name == name).expect("saved"),
        other => panic!("{other:?}"),
    }
}

async fn restore(h: &AppHandle) -> u32 {
    match h.call(AppCommand::RestoreSuggested).await.unwrap() {
        CommandOk::Restored { added } => added,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn first_run_seeds_presets_once() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    let list = frequencies(&h).await;
    assert_eq!(list.len(), 9, "region 1's suggestions: {list:?}");
    assert!(list.iter().all(|e| e.suggested && e.region == Some(1) && e.band.is_some()));
    assert_eq!(list.iter().find(|e| e.freq_hz == 7_047_000).and_then(|e| e.band.as_deref()), Some("40m"));
    for e in &list {
        h.call(AppCommand::DeleteFrequency { id: e.id.clone() }).await.unwrap();
    }
    // Selecting the station again does not bring them back.
    let id = snapshot(&h).await.active_profile_id.unwrap();
    h.call(AppCommand::SelectProfile { id: None }).await.unwrap();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    assert!(frequencies(&h).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_suggested_reports_added_count() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    let mine = save_entry(&h, entry("Club net", 7_110_000)).await;
    assert!(!mine.suggested);
    let list = frequencies(&h).await;
    for e in list.iter().filter(|e| e.suggested).take(2) {
        h.call(AppCommand::DeleteFrequency { id: e.id.clone() }).await.unwrap();
    }
    assert_eq!(restore(&h).await, 2);
    assert_eq!(restore(&h).await, 0);
    assert!(frequencies(&h).await.iter().any(|e| e.id == mine.id), "custom entries are left alone");
    // Another region's suggestions are added beside them.
    h.call(AppCommand::SetRegion { region: 2 }).await.unwrap();
    assert_eq!(restore(&h).await, 9);
    assert!(h.call(AppCommand::SetRegion { region: 4 }).await.is_err());
}

async fn bad(h: &AppHandle, e: FrequencyEntry) {
    let err = h.call(AppCommand::SaveFrequency { entry: e }).await.unwrap_err();
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn entry_validation_rejects_control_chars_and_long_names() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    bad(&h, entry("bell\u{7}", 7_047_000)).await;
    bad(&h, entry(&"x".repeat(65), 7_047_000)).await;
    bad(&h, entry("  ", 7_047_000)).await;
    bad(&h, entry("Broadcast", 6_000_000)).await; // outside every band, not marked
    bad(&h, FrequencyEntry { mode: "XYZ".into(), ..entry("Odd", 7_047_000) }).await;
    let ok = save_entry(&h, FrequencyEntry { out_of_band: true, ..entry("MARS", 6_000_000) }).await;
    assert_eq!(ok.band, None);
    assert!(ok.out_of_band);
    save_entry(&h, entry(&"y".repeat(64), 14_090_000)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_to_entry_sets_freq_and_mode() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    let e =
        save_entry(&h, FrequencyEntry { mode: "USB".into(), passband_hz: Some(2700), ..entry("Net", 7_110_000) }).await;
    h.call(AppCommand::TuneToEntry { id: e.id.clone() }).await.unwrap();
    wait_for(&h, 3, "the entry", |_| rig.freq() == 7_110_000 && rig.mode() == ("USB".to_string(), 2700)).await;
    assert!(h.call(AppCommand::TuneToEntry { id: "nope".into() }).await.is_err());
}

static CLOCK_MIN: AtomicU32 = AtomicU32::new(0);

fn test_clock() -> NaiveTime {
    let m = CLOCK_MIN.load(Ordering::SeqCst);
    NaiveTime::from_hms_opt(m / 60, m % 60, 0).unwrap()
}

fn at(h: u32, m: u32) {
    CLOCK_MIN.store(h * 60 + m, Ordering::SeqCst);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_slot_blocked_by_activity_waits_then_skips() {
    let dir = tempfile::tempdir().unwrap();
    let rig = FakeRigctld::start().await;
    at(9, 59);
    let o = AppOptions {
        schedule_clock: Some(test_clock),
        schedule_tick: Some(Duration::from_millis(50)),
        ..opts(dir.path())
    };
    let (h, mut rx) = ready_opts(StationProfile::new_simulator("ZS6ABC"), rigctld(&rig), o).await;
    let a = save_entry(&h, entry("A", 7_047_000)).await;
    let b = save_entry(&h, entry("B", 14_090_000)).await;
    let c = save_entry(&h, entry("C", 21_095_000)).await;
    let slot = |t: &str, e: &FrequencyEntry| ScheduleSlot { start_utc: t.into(), entry_id: e.id.clone() };
    let schedule = Schedule { enabled: true, slots: vec![slot("10:00", &a), slot("10:05", &b), slot("10:10", &c)] };
    h.call(AppCommand::SetSchedule { schedule }).await.unwrap();
    let next = snapshot(&h).await.schedule_next.expect("a next change");
    assert_eq!((next.at_utc.as_str(), next.entry_name.as_str()), ("10:00", "A"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!rig.log().iter().any(|l| l.starts_with("+F")), "no QSY on enabling: {:?}", rig.log());

    h.call(AppCommand::Call { peer: "SIM1".into() }).await.unwrap();
    until_state(&mut rx, ModemState::Connected, 15).await;
    at(10, 0); // A is due, but the session is up: it waits
    tokio::time::sleep(Duration::from_millis(400)).await;
    at(10, 5); // B starts: A is skipped, B waits
    let skipped =
        |entries: &[ActivityEntry]| entries.iter().any(|e| e.text.contains("skipped") && e.text.contains('A'));
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        let CommandOk::Activity { entries } = h.call(AppCommand::Activity).await.unwrap() else { panic!() };
        if skipped(&entries) {
            break;
        }
        assert!(Instant::now() < end, "A was not reported skipped: {entries:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.call(AppCommand::Disconnect).await.unwrap();
    wait_for(&h, 15, "B once idle", |_| rig.freq() == 14_090_000).await;
    assert!(!rig.log().iter().any(|l| l == "+F 7047000"), "A never reached the rig: {:?}", rig.log());
    let next = snapshot(&h).await.schedule_next.expect("a next change");
    assert_eq!((next.at_utc.as_str(), next.entry_name.as_str()), ("10:10", "C"));
}

#[tokio::test(flavor = "multi_thread")]
async fn band_levels_read_back_what_was_stored() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    let levels = BandLevels { tx_gain_db: -2.5, tune_dbfs: -30.0 };
    h.call(AppCommand::SetBandLevels { band: "40m".into(), levels }).await.unwrap();
    let CommandOk::BandLevels { levels: all } = h.call(AppCommand::BandLevels).await.unwrap() else { panic!() };
    assert_eq!(all.get("40m"), Some(&levels));
    assert_eq!(all.len(), 1);
}

// ---- Task 11: the link test in Settings → Rig

async fn rig_test(h: &AppHandle, rig: RigConfig) -> Vec<RigTestStep> {
    match h.call(AppCommand::TestRig { config: rig }).await.unwrap() {
        CommandOk::RigTest { steps } => steps,
        other => panic!("{other:?}"),
    }
}

/// An app whose own rig link is off, so only the test talks to the fake.
async fn no_rig_app(dir: &Path, net: &FakeNet) -> AppHandle {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir));
    let id = match h.call(AppCommand::SaveProfile { profile: existing(net) }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    until_state(&mut rx, ModemState::Listening, 15).await;
    h
}

fn names(steps: &[RigTestStep]) -> Vec<&str> {
    steps.iter().map(|s| s.name.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn rig_test_reports_each_step() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let h = no_rig_app(dir.path(), &net).await;
    let steps = rig_test(&h, rigctld(&rig)).await;
    assert_eq!(names(&steps), ["connect", "caps", "frequency", "mode", "ptt", "owner"]);
    for s in &steps[..5] {
        assert_eq!(s.ok, Some(true), "{s:?}");
    }
    assert!(steps[2].detail.contains("14.074"), "{:?}", steps[2]);
    assert!(steps[3].detail.contains("PKTUSB"), "{:?}", steps[3]);
    assert_eq!(steps[5].ok, None, "no telemetry: Mercury's PTT settings are unknown");
}

#[tokio::test(flavor = "multi_thread")]
async fn rig_test_explains_rprt_11() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    rig.ptt_error(Some(-11));
    let h = no_rig_app(dir.path(), &net).await;
    let steps = rig_test(&h, rigctld(&rig)).await;
    let ptt = steps.iter().find(|s| s.name == "ptt").unwrap();
    assert_eq!(ptt.ok, Some(false));
    assert!(ptt.detail.contains("-P RIG"), "{ptt:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn rig_test_with_nothing_listening_fails_at_connect_and_skips_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let cfg = rigctld(&rig);
    drop(rig);
    let h = no_rig_app(dir.path(), &net).await;
    let steps = rig_test(&h, cfg).await;
    assert_eq!(steps[0].ok, Some(false), "{:?}", steps[0]);
    for s in &steps[1..5] {
        assert_eq!(s.ok, None, "{s:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rig_test_never_writes() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let h = no_rig_app(dir.path(), &net).await;
    rig_test(&h, rigctld(&rig)).await;
    assert_eq!(rig.log(), [r"+\dump_caps", "+f", "+m", "+t"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn rig_settings_read_back_config_region_and_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rig = FakeRigctld::start().await;
    let (h, _rx) = ready(dir.path(), existing(&net), rigctld(&rig)).await;
    h.call(AppCommand::SetRegion { region: 3 }).await.unwrap();
    let e = save_entry(&h, entry("Net", 7_110_000)).await;
    let schedule = Schedule { enabled: true, slots: vec![ScheduleSlot { start_utc: "06:00".into(), entry_id: e.id }] };
    h.call(AppCommand::SetSchedule { schedule: schedule.clone() }).await.unwrap();
    let CommandOk::RigSettings { config, region, schedule: got } = h.call(AppCommand::RigSettings).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(config, rigctld(&rig));
    assert_eq!(region, 3);
    assert_eq!(got, schedule);
}

// ---- M3b.1 Task 5: MercuryChat's own rigctld, FLRig, the offset ------------

mod m3b1 {
    use super::*;
    use mc_core::app::{CatLink, FreqControl, PttMethod};
    use mc_core::profile::ModemTarget;
    use mercury_adapter::fake_flrig::FakeFlrig;
    use std::net::SocketAddr;
    use std::path::PathBuf;

    /// mercury-adapter's fake-rigctld program, built on demand (it is another
    /// package's binary, so Cargo does not hand its path to these tests).
    fn fake_rigctld() -> PathBuf {
        let exe = if cfg!(windows) { "fake-rigctld.exe" } else { "fake-rigctld" };
        // The profile folder (target/debug), where Cargo puts built programs:
        // the ancestor with Cargo's build lock (newer layout) or deps/ (older).
        let me = std::env::current_exe().unwrap();
        let dir = me
            .ancestors()
            .find(|a| a.join(".cargo-build-lock").is_file() || a.join("deps").is_dir())
            .expect("target profile folder")
            .to_path_buf();
        let p = dir.join(exe);
        if !p.is_file() {
            let ok = std::process::Command::new(env!("CARGO"))
                .args(["build", "-p", "mercury-adapter", "--bin", "fake-rigctld"])
                .status()
                .unwrap()
                .success();
            assert!(ok, "build fake-rigctld");
        }
        assert!(p.is_file(), "{}", p.display());
        p
    }

    fn free_addr() -> SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
    }

    async fn attached_with(
        o: AppOptions,
        profile: StationProfile,
        rig: RigConfig,
    ) -> (AppHandle, Receiver<EventEnvelope>) {
        let h = spawn_app(Store::open_in_memory().unwrap(), o);
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

    fn managed() -> RigConfig {
        let mut c = RigConfig { freq_control: FreqControl::ManagedRigctld, ..RigConfig::default() };
        c.cat.model = Some(1);
        c.cat.link = CatLink::Serial;
        c.cat.serial_port = "COM10".into();
        c.ptt.method = PttMethod::Cat;
        c
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn managed_starts_daemon_and_reads() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let o = AppOptions {
            rigctld_program: Some(Some(fake_rigctld())),
            managed_listen: Some(free_addr()),
            ..opts(dir.path())
        };
        let (h, _rx) = attached_with(o, existing(&net), managed()).await;
        let s = wait_for(&h, 15, "the managed rig", |s| {
            s.rig.link == RigLinkView::Up && s.rig.frequency.is_some() && s.rig.daemon.as_deref() == Some("running")
        })
        .await;
        assert_eq!(s.rig.frequency.unwrap().value, 14_074_000);
        match h.call(AppCommand::CatLog).await.unwrap() {
            CommandOk::CatLog { lines } => {
                assert!(lines.iter().any(|l| l.contains("fake-rigctld: ready")), "{lines:?}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn managed_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let o = AppOptions { rigctld_program: Some(None), managed_listen: Some(free_addr()), ..opts(dir.path()) };
        let (h, _rx) = attached_with(o, existing(&net), managed()).await;
        let s = wait_for(&h, 5, "not found", |s| s.rig.daemon.as_deref() == Some("notFound")).await;
        assert_ne!(s.rig.link, RigLinkView::Up);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn flrig_backend_reads() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let fl = FakeFlrig::start().await;
        fl.set_freq(10_142_000);
        let mut c = RigConfig { freq_control: FreqControl::Flrig, ..RigConfig::default() };
        c.flrig.port = fl.addr.port();
        let (h, _rx) = attached_with(opts(dir.path()), existing(&net), c).await;
        wait_for(&h, 5, "FLRig's frequency", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 10_142_000)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offset_applied_both_ways() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let rig = FakeRigctld::start().await;
        rig.set_freq(14_106_500);
        let mut c = rigctld(&rig);
        c.offset_hz = -1500;
        let (h, _rx) = ready(dir.path(), existing(&net), c).await;
        wait_for(&h, 3, "the shown frequency", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 14_105_000))
            .await;
        set(&h, RigChange::Frequency { hz: 7_105_000 }).await.unwrap();
        wait_for(&h, 3, "the rig's dial", |_| rig.freq() == 7_106_500).await;
        assert!(rig.log().iter().any(|l| l.ends_with("F 7106500")), "{:?}", rig.log());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offset_below_zero_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let rig = FakeRigctld::start().await;
        let mut c = rigctld(&rig);
        c.offset_hz = 2000;
        let (h, _rx) = ready(dir.path(), existing(&net), c).await;
        let before = rig.log().len();
        match set(&h, RigChange::Frequency { hz: 1000 }).await {
            Err(AppError::Validation { errors }) => assert_eq!(errors[0].field, "frequency"),
            other => panic!("{other:?}"),
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!rig.log()[before..].iter().any(|l| l.contains("F ")), "{:?}", rig.log());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_every_passed() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let rig = FakeRigctld::start().await;
        let mut c = rigctld(&rig);
        c.read_every_s = 5;
        let (_h, _rx) = ready(dir.path(), existing(&net), c).await;
        tokio::time::sleep(Duration::from_millis(11_500)).await;
        let reads = rig.log().iter().filter(|l| l.trim_start_matches('+') == "f").count();
        assert!((2..=3).contains(&reads), "{reads} frequency reads in 11.5 s at every 5 s: {:?}", rig.log());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hamlib_models_lists_or_says_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let o = AppOptions { rigctld_program: Some(None), ..opts(dir.path()) };
        let h = spawn_app(Store::open_in_memory().unwrap(), o);
        match h.call(AppCommand::HamlibModels { hamlib_dir: String::new() }).await.unwrap() {
            CommandOk::HamlibModels { models, found } => assert!(!found && models.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    // ---- Task 7: Mercury's PTT set to match --------------------------------

    use mercury_adapter::fake_ws::FakeWs;

    const STATUS: &str = r#"{"type":"status","audio_ok":true}"#;
    const MERCURY_PTT_NONE: &str = r#"{"type":"radio_list","selected":"0","device_path":"","serial_speed":0,"ptt_method":"none","ptt_line":"rts","ptt_invert":"none","cm108_gpio":3,"list":[]}"#;

    fn with_ws(net: &FakeNet, ws: &FakeWs) -> StationProfile {
        let mut p = existing(net);
        if let ModemTarget::Existing { telemetry_url, .. } = &mut p.modem {
            *telemetry_url = Some(ws.url.clone());
        }
        p
    }

    async fn fake_mercury_ws() -> FakeWs {
        let ws = FakeWs::start().await;
        ws.set_status(Some(STATUS));
        ws.set_radio_list(Some(MERCURY_PTT_NONE));
        ws
    }

    fn ptt_sent(ws: &FakeWs) -> Vec<serde_json::Value> {
        ws.received()
            .iter()
            .filter_map(|t| serde_json::from_str::<serde_json::Value>(t).ok())
            .filter(|v| v["command"] == "set_ptt_config")
            .collect()
    }

    async fn ws_until(ws: &FakeWs, secs: u64, what: &str, pred: impl Fn(&FakeWs) -> bool) {
        let end = Instant::now() + Duration::from_secs(secs);
        while !pred(ws) {
            assert!(Instant::now() < end, "timed out waiting for {what}: {:?}", ws.received());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn external_cat(rig: &FakeRigctld) -> RigConfig {
        let mut c = rigctld(rig);
        c.ptt.method = PttMethod::Cat;
        c
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ptt_pushed_on_connect_once() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let rig = FakeRigctld::start().await;
        let ws = fake_mercury_ws().await;
        let (h, _rx) = attached(dir.path(), with_ws(&net, &ws), external_cat(&rig)).await;
        ws_until(&ws, 10, "one set_ptt_config", |ws| !ptt_sent(ws).is_empty()).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let sent = ptt_sent(&ws);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0]["value"], "hamlib");
        assert_eq!(sent[0]["value2"], format!("127.0.0.1:{}", rig.addr.port()));
        assert_eq!(sent[0]["value3"], "2");
        // Mercury now keys through MercuryChat's rigctld: the owner check agrees.
        wait_for(&h, 5, "the owner check", |s| s.rig.owner_check == mc_core::app::OwnerCheck::Ok).await;
        // A reconnect brings Mercury's (now matching) radio_list: nothing more is sent.
        ws.kill();
        ws_until(&ws, 30, "a reconnect", |ws| ws.accepted() >= 2).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(ptt_sent(&ws).len(), 1, "{:?}", ptt_sent(&ws));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn leave_mercury_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let rig = FakeRigctld::start().await;
        let ws = fake_mercury_ws().await;
        let (_h, _rx) = attached(dir.path(), with_ws(&net, &ws), rigctld(&rig)).await;
        ws_until(&ws, 10, "the UI link", |ws| ws.accepted() >= 1).await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(ptt_sent(&ws).is_empty(), "{:?}", ptt_sent(&ws));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ptt_push_deferred_while_busy() {
        let dir = tempfile::tempdir().unwrap();
        let net = FakeNet::start().await;
        let rig = FakeRigctld::start().await;
        let ws = fake_mercury_ws().await;
        let (h, _rx) = attached(dir.path(), with_ws(&net, &ws), rigctld(&rig)).await;
        ws_until(&ws, 10, "the UI link", |ws| ws.accepted() >= 1).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        net.set_unanswered_calls(true); // the call stays pending, as Mercury retries
        h.call(AppCommand::Call { peer: "ZS1XYZ".into() }).await.unwrap();
        wait_for(&h, 5, "calling", |s| s.modem == ModemState::Calling).await;
        h.call(AppCommand::SetRigConfig { config: external_cat(&rig) }).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let still = snapshot(&h).await.modem == ModemState::Calling;
        let sent = ptt_sent(&ws);
        assert!(still, "the call ended before the check could run");
        assert!(sent.is_empty(), "not while calling: {sent:?}");
        h.call(AppCommand::CancelCall).await.unwrap();
        ws_until(&ws, 10, "the deferred set_ptt_config", |ws| ptt_sent(ws).len() == 1).await;
    }
}
