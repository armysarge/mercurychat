//! M3b rig lab (lab/docker/rig/run-m3b-lab.sh): the real service actor,
//! headless, against two real Mercury instances that key PTT through Hamlib
//! rigctld (the dummy rig: no transceiver, no serial port, no RF).
//!
//!   Station A (ours): TNC 127.0.0.1:8300, telemetry ws://127.0.0.1:10000, rigctld 127.0.0.1:4532
//!   Station B (peer): TNC 127.0.0.1:8400, answered by this program's own headless peer
//!
//! It prints one `PASS|FAIL <check>: <detail>` line per check (05 §3 M3 exit as
//! amended, design §7.9) and exits non-zero on any FAIL. The rigctld for A is
//! killed and restarted through shell commands the script passes in:
//! `RIG_KILL`, `RIG_START` (with `-P RIG`) and `RIG_START_NOP` (without `-P`),
//! and check 9 starts a third Mercury with a serial Hamlib model through
//! `MERCURY_C_START` (telemetry on `MERCURY_C_WS`, TNC on 8500).
//! `RIG_LOG` is rigctld A's `-vvvv` log, read to see what reached the rig.

use mc_core::app::{
    spawn_app, ActivityEntry, AppCommand, AppHandle, AppOptions, AppSnapshot, BandLevels, CommandOk, ModemState,
    OwnerCheck, RigBanner, RigChange, RigConfig, RigLinkView, RigPttView, TxLevelResult,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_storage::Store;
use mercury_adapter::{Callsign, Endpoint};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::Instant;

struct Lab {
    h: AppHandle,
    out: PathBuf,
    results: Vec<String>,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

fn shell(cmd: &str) {
    let status = std::process::Command::new("bash").arg("-c").arg(cmd).status().expect("bash");
    if !status.success() {
        eprintln!("command failed ({status}): {cmd}");
    }
}

fn rig(port: u16) -> RigConfig {
    RigConfig::external("127.0.0.1", port)
}

fn profile(call: &str, tnc: u16, ws: u16) -> StationProfile {
    let mut p = StationProfile::new_existing(call);
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: tnc,
        data_port: tnc + 1,
        broadcast_port: None,
        telemetry_url: Some(format!("ws://127.0.0.1:{ws}/websocket")),
    };
    p
}

impl Lab {
    async fn snap(&self) -> AppSnapshot {
        match self.h.call(AppCommand::Snapshot).await.expect("snapshot") {
            CommandOk::Snapshot(s) => s,
            other => panic!("{other:?}"),
        }
    }

    /// Poll until `pred` holds; the last snapshot either way.
    async fn wait(&self, secs: f64, pred: impl Fn(&AppSnapshot) -> bool) -> (bool, AppSnapshot) {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        loop {
            let s = self.snap().await;
            if pred(&s) {
                return (true, s);
            }
            if Instant::now() >= end {
                return (false, s);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn activity(&self) -> Vec<ActivityEntry> {
        match self.h.call(AppCommand::Activity).await.expect("activity") {
            CommandOk::Activity { entries } => entries,
            other => panic!("{other:?}"),
        }
    }

    fn rig_log_count(&self, needle: &str) -> usize {
        std::fs::read_to_string(env("RIG_LOG")).unwrap_or_default().matches(needle).count()
    }

    fn record(&mut self, name: &str, ok: bool, detail: impl AsRef<str>) {
        let line = format!("{} {name}: {}", if ok { "PASS" } else { "FAIL" }, detail.as_ref());
        println!("{line}");
        self.results.push(line);
    }

    async fn rig_up(&self) -> bool {
        self.wait(10.0, |s| s.rig.link == RigLinkView::Up && s.rig.ptt != RigPttView::Unknown).await.0
    }

    async fn idle(&self) -> bool {
        self.wait(30.0, |s| matches!(s.modem, ModemState::Ready | ModemState::Listening)).await.0
    }
}

/// Watch the snapshot until `stop`, noting whether the rig's PTT was seen on
/// and whether any fault was raised.
async fn watch(h: AppHandle, stop: tokio::sync::watch::Receiver<bool>) -> (bool, Vec<RigBanner>) {
    let (mut keyed, mut faults) = (false, vec![]);
    while !*stop.borrow() {
        if let Ok(CommandOk::Snapshot(s)) = h.call(AppCommand::Snapshot).await {
            keyed |= s.rig.ptt == RigPttView::On;
            if let Some(f) = s.rig.fault {
                if !faults.contains(&f) {
                    faults.push(f);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    (keyed, faults)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "/out".into()));
    let data = out.join("app");
    std::fs::create_dir_all(&data).unwrap();

    // Station B: a headless peer that answers calls and echoes chats.
    let mut peer_cfg = StationConfig::defaults(
        Callsign::parse("ZS6PEER").unwrap(),
        Endpoint::base("127.0.0.1", 8400),
        &data.join("peer"),
    );
    // Over the air a HELLO takes a turn or two of several seconds (the defaults are the simulator's).
    peer_cfg.hello_timeout = mc_core::station::RADIO_HELLO_TIMEOUT;
    let mut peer = Station::open(peer_cfg).expect("peer station");
    peer.attach().await.expect("peer attach");
    peer.set_listen(true).await.expect("peer listen");
    tokio::spawn(mc_core::sim::serve_peer(peer, true, None));

    let opts = AppOptions { stations_root: Some(data.join("stations")), ..AppOptions::default() };
    let h = spawn_app(Store::open_in_memory().unwrap(), opts);
    let mut lab = Lab { h: h.clone(), out: out.clone(), results: vec![] };
    let id = match h.call(AppCommand::SaveProfile { profile: profile("ZS6LAB", 8300, 10000) }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    assert!(lab.wait(30.0, |s| s.modem == ModemState::Listening).await.0, "station A did not attach");
    h.call(AppCommand::SetRigConfig { config: rig(4532) }).await.unwrap();
    assert!(lab.rig_up().await, "the rig link did not come up");
    let (_, s) =
        lab.wait(10.0, |s| s.tune.supported.is_some() && !matches!(s.rig.owner_check, OwnerCheck::Unknown)).await;
    println!(
        "INFO setup: rig {:?}, owner check {:?}, tune {:?}",
        s.rig.caps.as_ref().map(|c| &c.model),
        s.rig.owner_check,
        s.tune.supported
    );

    // ---- 1 and 3: a session keys the rig through rigctld; no change reaches the rig meanwhile.
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let watcher = tokio::spawn(watch(h.clone(), stop_rx));
    h.call(AppCommand::Call { peer: "ZS6PEER".into() }).await.unwrap();
    let (up, _) = lab.wait(120.0, |s| s.modem == ModemState::Connected).await;
    let mut refused = String::new();
    let sets_before = lab.rig_log_count("rig_set_freq");
    if up {
        let r = h.call(AppCommand::RigSet { change: RigChange::Frequency { hz: 7_110_000 } }).await;
        refused = format!("{r:?}");
        let _ = h.call(AppCommand::SendChat { peer: "ZS6PEER".into(), text: "rig lab".into() }).await;
        tokio::time::sleep(Duration::from_secs(15)).await;
        let _ = h.call(AppCommand::Disconnect).await;
    }
    lab.idle().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    stop_tx.send(true).unwrap();
    let (keyed, faults) = watcher.await.unwrap();
    let ok1 = up && keyed && faults.is_empty() && matches!(lab.snap().await.rig.owner_check, OwnerCheck::Ok);
    lab.record("1 shared rigctld", ok1, format!("session {up}; rig PTT seen on {keyed}; faults {faults:?}"));
    let sets_after = lab.rig_log_count("rig_set_freq");
    let ok3 = up && refused.contains("rig.refused.connected") && sets_after == sets_before;
    lab.record(
        "3 refused while connected",
        ok3,
        format!("{refused}; rig_set_freq in rigctld log {sets_before} -> {sets_after}"),
    );

    // ---- 2: frequency and mode while idle; Mercury's telemetry follows.
    let a = h.call(AppCommand::RigSet { change: RigChange::Frequency { hz: 7_074_000 } }).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let b = h.call(AppCommand::RigSet { change: RigChange::Mode { mode: "PKTUSB".into(), passband_hz: 3000 } }).await;
    let t0 = Instant::now();
    // Mercury re-reads the rig on its own schedule: the bound is 10 s, and the time it took is reported.
    let (follows, s) = lab.wait(10.0, |s| s.telemetry.frequency.as_ref().is_some_and(|f| f.value == 7_074_000)).await;
    let mode = lab.wait(3.0, |s| s.rig.mode.as_deref() == Some("PKTUSB")).await.0;
    lab.record(
        "2 set while idle",
        a.is_ok() && b.is_ok() && follows && mode,
        format!(
            "set {a:?}/{b:?}; telemetry {:?} after {:.1} s; rig mode PKTUSB {mode}",
            s.telemetry.frequency.map(|f| f.value),
            t0.elapsed().as_secs_f32()
        ),
    );

    // ---- 6: TX level on a band change, confirmed from Mercury's status.
    let levels = BandLevels { tx_gain_db: -3.0, tune_dbfs: -20.0 };
    h.call(AppCommand::SetBandLevels { band: "20m".into(), levels }).await.unwrap();
    h.call(AppCommand::RigSet { change: RigChange::Frequency { hz: 14_090_000 } }).await.unwrap();
    let (applied, s) = lab
        .wait(12.0, |s| matches!(&s.rig.tx_level_applied, Some(TxLevelResult::Applied { band, .. }) if band == "20m"))
        .await;
    lab.record("6 TX level per band", applied, format!("{:?}", s.rig.tx_level_applied));

    // ---- 7: Tune keys the rig and stops by itself within 30 s.
    let started = Instant::now();
    let t = h.call(AppCommand::Tune { on: true }).await;
    let (keyed, _) = lab.wait(5.0, |s| s.tune.active && s.rig.ptt == RigPttView::On).await;
    let (stopped, s) = lab.wait(36.0, |s| !s.tune.active && s.rig.ptt == RigPttView::Off).await;
    let took = started.elapsed().as_secs_f32();
    lab.record(
        "7 tune",
        t.is_ok() && keyed && stopped && took <= 33.0 && s.rig.fault.is_none(),
        format!("start {t:?}; rig keyed {keyed}; stopped {stopped} after {took:.1} s; fault {:?}", s.rig.fault),
    );

    // ---- 4: rigctld killed mid-transmission.
    let before = lab.activity().await.len();
    h.call(AppCommand::Tune { on: true }).await.ok();
    lab.wait(5.0, |s| s.rig.ptt == RigPttView::On).await;
    shell(&env("RIG_KILL"));
    let (banner, _) = lab.wait(10.0, |s| s.rig.banners.contains(&RigBanner::LostWhileKeyed)).await;
    shell(&env("RIG_START"));
    lab.rig_up().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    h.call(AppCommand::Tune { on: false }).await.ok();
    lab.wait(10.0, |s| !s.tune.active).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let log = lab.activity().await;
    let after: Vec<&ActivityEntry> = log.iter().skip(before).collect();
    let offs = after.iter().filter(|e| e.text.contains("sent PTT off once")).count();
    // Mercury's PTT state at each point, from its own PTT lines in the same log.
    let mut mercury_on = false;
    let mut during = 0;
    for e in &after {
        if e.kind == "ptt" {
            mercury_on = e.text.eq_ignore_ascii_case("PTT on");
        } else if e.text.contains("sent PTT off once") && mercury_on {
            during += 1;
        }
    }
    lab.record(
        "4 killed while keyed",
        banner && offs <= 1 && during == 0,
        format!("LostWhileKeyed {banner}; our PTT off sent {offs} time(s), {during} while Mercury reported PTT on"),
    );
    let _ = h.call(AppCommand::DismissRigBanner { banner: RigBanner::LostWhileKeyed }).await;

    // ---- 5: rigctld killed while idle: the modem carries on, the frequency falls back to telemetry.
    lab.idle().await;
    shell(&env("RIG_KILL"));
    let (lost, s) = lab.wait(10.0, |s| matches!(s.rig.link, RigLinkView::Lost { .. })).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let s2 = lab.snap().await;
    let ok5 = lost && matches!(s2.modem, ModemState::Ready | ModemState::Listening) && s2.telemetry.frequency.is_some();
    lab.record(
        "5 killed while idle",
        ok5,
        format!(
            "link {:?}; modem {:?}; telemetry frequency {:?}",
            s.rig.link,
            s2.modem,
            s2.telemetry.frequency.map(|f| f.value)
        ),
    );

    // ---- 8: rigctld without -P: the PTT check is off, no fault, Tune refused.
    shell(&env("RIG_START_NOP"));
    let (off, s) = lab.wait(15.0, |s| s.rig.link == RigLinkView::Up && s.rig.ptt_check_off).await;
    let tune = h.call(AppCommand::Tune { on: true }).await;
    let refused = format!("{tune:?}");
    let ok8 = off && s.rig.fault.is_none() && refused.contains("rig.refused.pttUnknown");
    lab.record("8 no -P", ok8, format!("ptt_check_off {off}; fault {:?}; Tune {refused}", s.rig.fault));
    shell(&env("RIG_KILL"));
    shell(&env("RIG_START"));
    lab.rig_up().await;

    // ---- 9: a Mercury with its own serial Hamlib model: rig control refused with the reason.
    shell(&env("MERCURY_C_START"));
    tokio::time::sleep(Duration::from_secs(3)).await;
    let ws: u16 = env("MERCURY_C_WS").parse().unwrap();
    let id_c = match h.call(AppCommand::SaveProfile { profile: profile("ZS6LBC", 8500, ws) }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id_c) }).await.unwrap();
    lab.wait(30.0, |s| matches!(s.modem, ModemState::Ready | ModemState::Listening)).await;
    h.call(AppCommand::SetRigConfig { config: rig(4532) }).await.unwrap();
    let (two, s) = lab.wait(15.0, |s| matches!(s.rig.owner_check, OwnerCheck::Refused { .. })).await;
    let set = h.call(AppCommand::RigSet { change: RigChange::Frequency { hz: 7_100_000 } }).await;
    lab.record(
        "9 two owners",
        two && set.is_err() && !matches!(s.rig.link, RigLinkView::Up),
        format!("owner check {:?}; link {:?}; a change {set:?}", s.rig.owner_check, s.rig.link),
    );

    let log: Vec<String> =
        lab.activity().await.iter().map(|e| format!("{} [{}] {}", e.at_utc, e.kind, e.text)).collect();
    std::fs::write(lab.out.join("activity.txt"), log.join("\n") + "\n").unwrap();
    let failed = lab.results.iter().filter(|r| r.starts_with("FAIL")).count();
    let report =
        format!("{}\n{} of {} passed\n", lab.results.join("\n"), lab.results.len() - failed, lab.results.len());
    std::fs::write(lab.out.join("report.txt"), &report).unwrap();
    print!("{report}");
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
