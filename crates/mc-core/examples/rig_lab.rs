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
//!
//! M3b.1 (lab/docker/rig/run-rig-lab-checks.sh, lab/docker/rig/M3b1-checks.md):
//! `rig_lab <out> m3b1` runs against a Mercury A started with `-P none` and no
//! rigctld on 4532: MercuryChat starts its own (managed), sets Mercury's PTT,
//! shows the offset, and sets Mercury's audio. `rig_lab <out> reuse` finds a
//! rigctld already on 4532 and uses it. `WS_SNOOP` is a command that prints
//! Mercury's own view (ws-snoop.py); `MERCURY_A_INI` is Mercury A's ini.
//!
//! M3c (lab/docker/rig/M3c-checks.md): `rig_lab <out> m3c-qsy` runs two app
//! instances, A and B, each with its own external dummy rigctld (4532 and
//! 4632, offsets 0 and +1500): QSY, its fallback and gate, info requests, and
//! MCB/1 kinds 2 and 3. `RIG_KILL`, `RIG_START` and `RIG_START_NOP` act on A's.

use mc_core::app::{
    spawn_app, ActivityEntry, AppCommand, AppEvent, AppHandle, AppOptions, AppSnapshot, AudioSettingsView, BandLevels,
    CommandOk, EventEnvelope, FreqControl, FrequencyEntry, HeardStation, ModemState, OwnerCheck, PttMethod, Reading,
    RigBanner, RigChange, RigConfig, RigLinkView, RigPttView, TxLevelResult,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_storage::Store;
use mch_protocol::payload::InfoWhat;
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

/// Station B: a headless peer that answers calls and echoes chats.
async fn start_peer(data: &std::path::Path) {
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
}

/// The app with station A selected and attached.
async fn start_lab(out: &std::path::Path, data: &std::path::Path) -> Lab {
    let opts = AppOptions { stations_root: Some(data.join("stations")), ..AppOptions::default() };
    let h = spawn_app(Store::open_in_memory().unwrap(), opts);
    let lab = Lab { h: h.clone(), out: out.to_path_buf(), results: vec![] };
    let id = match h.call(AppCommand::SaveProfile { profile: profile("ZS6LAB", 8300, 10000) }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    assert!(lab.wait(30.0, |s| s.modem == ModemState::Listening).await.0, "station A did not attach");
    lab
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "/out".into()));
    let mode = std::env::args().nth(2).unwrap_or_else(|| "m3b".into());
    let data = out.join("app");
    std::fs::create_dir_all(&data).unwrap();
    let lab = match mode.as_str() {
        "m3b" => {
            start_peer(&data).await;
            m3b(start_lab(&out, &data).await).await
        }
        "m3b1" => {
            start_peer(&data).await;
            m3b1(start_lab(&out, &data).await).await
        }
        "reuse" => reuse(start_lab(&out, &data).await).await,
        "m3c-qsy" => m3c(&out, &data).await,
        other => panic!("unknown mode {other}: m3b, m3b1, reuse or m3c-qsy"),
    };
    lab.finish(&mode).await
}

/// The M3b exit checks (1-9).
async fn m3b(mut lab: Lab) -> Lab {
    let h = lab.h.clone();
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

    lab
}

/// The managed rigctld's configuration for station A (M3b.1 checks 1, 3, 5).
fn managed(offset_hz: i32) -> RigConfig {
    let mut c = RigConfig { freq_control: FreqControl::ManagedRigctld, offset_hz, ..RigConfig::default() };
    c.cat.model = Some(1); // Hamlib's dummy rig: no serial port, no radio
                           // The dummy rig opens no port, but the settings need one: nothing is a device here.
    c.cat.serial_port = "/dev/null".into();
    c.hamlib_dir = "/usr/bin".into();
    c.ptt.method = PttMethod::Cat;
    c
}

/// Mercury's own view (ws-snoop.py), as JSON.
fn snoop() -> serde_json::Value {
    let out = std::process::Command::new("bash").arg("-c").arg(env("WS_SNOOP")).output().expect("ws snoop");
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        eprintln!("ws snoop: {e}: {}", String::from_utf8_lossy(&out.stdout));
        serde_json::Value::Null
    })
}

/// Lines of `pgrep -a rigctld` that listen on 4532.
fn rigctld_4532() -> Vec<String> {
    let out = std::process::Command::new("pgrep").args(["-a", "rigctld"]).output().expect("pgrep");
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("4532")).map(String::from).collect()
}

impl Lab {
    async fn audio(&self) -> AudioSettingsView {
        match self.h.call(AppCommand::AudioSettings).await.expect("audio settings") {
            CommandOk::Audio(v) => v,
            other => panic!("{other:?}"),
        }
    }

    async fn audio_until(&self, secs: f64, pred: impl Fn(&AudioSettingsView) -> bool) -> (bool, AudioSettingsView) {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        loop {
            let v = self.audio().await;
            if pred(&v) || Instant::now() >= end {
                return (pred(&v), v);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn daemon(&self, secs: f64, want: &str) -> (bool, AppSnapshot) {
        self.wait(secs, |s| s.rig.daemon.as_deref() == Some(want) && s.rig.link == RigLinkView::Up).await
    }

    /// Stop what the app started (as the window does on exit), write the
    /// activity log and the report; exit non-zero on any FAIL.
    async fn finish(self, mode: &str) -> ! {
        let stopped = tokio::time::timeout(Duration::from_secs(10), self.h.call(AppCommand::Shutdown)).await;
        println!("INFO shutdown: {stopped:?}");
        let log: Vec<String> =
            self.activity().await.iter().map(|e| format!("{} [{}] {}", e.at_utc, e.kind, e.text)).collect();
        std::fs::write(self.out.join(format!("activity-{mode}.txt")), log.join("\n") + "\n").unwrap();
        let failed = self.results.iter().filter(|r| r.starts_with("FAIL")).count();
        let report =
            format!("{}\n{} of {} passed\n", self.results.join("\n"), self.results.len() - failed, self.results.len());
        std::fs::write(self.out.join(format!("report-{mode}.txt")), &report).unwrap();
        print!("{report}");
        std::process::exit(if failed == 0 { 0 } else { 1 });
    }
}

/// M3b.1 checks 1, 3, 4 and 5 (M3b1-checks.md); 1's "nothing left after
/// exit" and check 2 are the script's.
async fn m3b1(mut lab: Lab) -> Lab {
    let h = lab.h.clone();

    // ---- 1 managed: MercuryChat starts rigctld itself and reads the rig through it.
    let before = rigctld_4532();
    h.call(AppCommand::SetRigConfig { config: managed(0) }).await.unwrap();
    let (running, s) = lab.daemon(15.0, "running").await;
    let up = lab.rig_up().await;
    let ours = rigctld_4532();
    lab.record(
        "1 managed rigctld",
        before.is_empty() && running && up && ours.len() == 1 && ours[0].contains("-m 1"),
        format!("before {before:?}; daemon {:?}; link up {up}; process {ours:?}", s.rig.daemon),
    );

    // ---- 5 offset: the dummy rig's dial at 14 100 000 shows as 14 101 500.
    h.call(AppCommand::SetRigConfig { config: managed(1500) }).await.unwrap();
    lab.daemon(15.0, "running").await;
    lab.rig_up().await;
    shell("rigctl -m 2 -r 127.0.0.1:4532 F 14100000");
    let (shown, s) = lab.wait(10.0, |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 14_101_500)).await;
    lab.record("5 offset", shown, format!("dial 14100000 + 1500 -> view {:?}", s.rig.frequency.map(|f| f.value)));

    // ---- 3 PTT push: Mercury (started with -P none) is set to key through our rigctld.
    let (_, s) = lab.wait(15.0, |_| snoop()["radio_list"]["ptt_method"] == "hamlib").await;
    let mut rl = snoop()["radio_list"].clone();
    if let Some(o) = rl.as_object_mut() {
        o.remove("list"); // Hamlib's whole model list
    }
    let pushed = rl["ptt_method"] == "hamlib" && rl["device_path"] == "127.0.0.1:4532";
    let noted = lab.activity().await.iter().any(|e| e.text.contains("set Mercury's PTT"));
    // A separate rigctl client watches the rig's PTT through the session.
    let mut poll = std::process::Command::new("bash")
        .arg("-c")
        .arg("while :; do rigctl -m 2 -r 127.0.0.1:4532 t; sleep 0.1; done")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("rigctl poll");
    h.call(AppCommand::Call { peer: "ZS6PEER".into() }).await.unwrap();
    let (session, _) = lab.wait(120.0, |s| s.modem == ModemState::Connected).await;
    if session {
        let _ = h.call(AppCommand::SendChat { peer: "ZS6PEER".into(), text: "M3b.1 rig lab".into() }).await;
        tokio::time::sleep(Duration::from_secs(15)).await;
        let _ = h.call(AppCommand::Disconnect).await;
    }
    lab.idle().await;
    let _ = poll.kill();
    let polled = String::from_utf8_lossy(&poll.wait_with_output().expect("rigctl poll output").stdout).to_string();
    let ones = polled.lines().filter(|l| l.trim() == "1").count();
    let zeros = polled.lines().filter(|l| l.trim() == "0").count();
    lab.record(
        "3 PTT push",
        pushed && noted && session && ones > 0,
        format!(
            "radio_list {rl}; activity note {noted}; session {session}; rigctl t saw 1 x{ones}, 0 x{zeros}; rig view PTT {:?}",
            s.rig.ptt
        ),
    );

    // ---- 4 audio: Mercury's devices, a change it accepts and saves, and one it refuses.
    let ini = || std::fs::read_to_string(env("MERCURY_A_INI")).unwrap_or_default();
    let (listed, v) = lab
        .audio_until(10.0, |v| {
            let ins: Vec<&str> = v.inputs.iter().map(|d| d.id.as_str()).collect();
            let outs: Vec<&str> = v.outputs.iter().map(|d| d.id.as_str()).collect();
            v.available && ins.contains(&"a_to_b.monitor") && outs.contains(&"b_to_a")
        })
        .await;
    let system = v.system.clone();
    let set = |input: &str, output: &str| AppCommand::SetAudio {
        system: system.clone(),
        input: input.into(),
        output: output.into(),
        channel: "left".into(),
    };
    let r1 = h.call(set("a_to_b.monitor", "b_to_a")).await;
    let (applied, v1) = lab
        .audio_until(15.0, |v| v.input == "a_to_b.monitor" && v.output == "b_to_a" && v.audio_ok == Some(true))
        .await;
    let saved = ini();
    let in_ini = saved.contains("a_to_b.monitor") && saved.contains("b_to_a");
    let r2 = h.call(set("mc_lab_no_such_source", "b_to_a")).await;
    let (refused, v2) = lab.audio_until(15.0, |v| v.audio_ok == Some(false)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let unchanged = ini() == saved;
    // Back to the crossed routing, so Mercury A hears B again.
    let r3 = h.call(set("b_to_a.monitor", "a_to_b")).await;
    let (back, _) = lab
        .audio_until(15.0, |v| v.input == "b_to_a.monitor" && v.output == "a_to_b" && v.audio_ok == Some(true))
        .await;
    let ids = |v: &AudioSettingsView| {
        format!(
            "in {:?} out {:?}",
            v.inputs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            v.outputs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>()
        )
    };
    lab.record(
        "4 audio",
        listed && r1.is_ok() && applied && in_ini && r2.is_ok() && refused && unchanged && r3.is_ok() && back,
        format!(
            "{} (system {system}); set {r1:?} -> audio_ok {:?}, ini holds both {in_ini}; bogus {r2:?} -> audio_ok {:?} ({:?}), ini unchanged {unchanged}; restored {back}",
            ids(&v),
            v1.audio_ok,
            v2.audio_ok,
            v2.audio_error
        ),
    );
    lab
}

/// M3b.1 check 2: a rigctld already on 4532 is used, and left running.
async fn reuse(mut lab: Lab) -> Lab {
    let before = rigctld_4532();
    lab.h.call(AppCommand::SetRigConfig { config: managed(0) }).await.unwrap();
    let (reused, s) = lab.daemon(15.0, "reused").await;
    lab.record(
        "2 reuse",
        before.len() == 1 && reused,
        format!("before {before:?}; daemon {:?}; link {:?}", s.rig.daemon, s.rig.link),
    );
    lab
}

// ---------------------------------------------------------------------------
// M3c (lab/docker/rig/M3c-checks.md): `rig_lab <out> m3c-qsy` runs two app
// instances, A (ZS6LAB: TNC 8300, telemetry 10000, broadcast 8100, rigctld
// 4532, offset 0) and B (ZS6PEER: TNC 8400, telemetry 10100, broadcast 8200,
// rigctld 4632, offset +1500), each with an external dummy rigctld. The
// QSY windows are the real ones (60 s answer, 120 s re-call, +20 s grace).

const B_OFFSET: i32 = 1500;

struct Side {
    lab: Lab,
    rx: tokio::sync::broadcast::Receiver<EventEnvelope>,
}

impl Side {
    /// The first event matching `pred` within `secs`.
    async fn event(&mut self, secs: f64, pred: impl Fn(&AppEvent) -> bool) -> Option<AppEvent> {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        loop {
            match tokio::time::timeout_at(end, self.rx.recv()).await {
                Ok(Ok(e)) if pred(&e.event) => return Some(e.event),
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(e)) => panic!("{e}"),
                Err(_) => return None,
            }
        }
    }

    /// Drop the events queued so far.
    fn drain(&mut self) {
        while self.rx.try_recv().is_ok() {}
    }

    async fn entry(&self, name: &str, hz: u64) -> String {
        let entry = FrequencyEntry {
            name: name.into(),
            freq_hz: hz,
            mode: "USB".into(),
            passband_hz: Some(2400),
            purpose: vec!["chat".into()],
            ..FrequencyEntry::default()
        };
        match self.lab.h.call(AppCommand::SaveFrequency { entry }).await.unwrap() {
            CommandOk::Frequencies { entries } => entries.into_iter().find(|e| e.name == name).unwrap().id,
            other => panic!("{other:?}"),
        }
    }

    async fn heard(&self) -> Vec<HeardStation> {
        match self.lab.h.call(AppCommand::Heard).await.unwrap() {
            CommandOk::Heard { stations } => stations,
            other => panic!("{other:?}"),
        }
    }

    async fn session_id(&self) -> Option<String> {
        self.lab.snap().await.session.map(|s| s.session_id)
    }
}

/// The dummy rig's dial, read by a separate rigctl client.
fn dial(port: u16) -> Option<u64> {
    let out = std::process::Command::new("rigctl")
        .args(["-m", "2", "-r", &format!("127.0.0.1:{port}"), "f"])
        .output()
        .ok()?;
    // rigctl prints a Hamlib banner line before the value.
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.trim().parse().ok())
}

async fn dial_is(port: u16, hz: u64, secs: f64) -> (bool, Option<u64>) {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    loop {
        let d = dial(port);
        if d == Some(hz) {
            return (true, d);
        }
        if Instant::now() >= end {
            return (false, d);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_side(
    out: &std::path::Path,
    data: &std::path::Path,
    call: &str,
    tnc: u16,
    ws: u16,
    bcast: u16,
    rig_port: u16,
    offset_hz: i32,
    station: impl FnOnce(&mut StationProfile),
) -> Side {
    let opts = AppOptions { stations_root: Some(data.join(call).join("stations")), ..AppOptions::default() };
    let h = spawn_app(Store::open_in_memory().unwrap(), opts);
    let rx = h.subscribe();
    let lab = Lab { h: h.clone(), out: out.to_path_buf(), results: vec![] };
    let mut p = profile(call, tnc, ws);
    if let ModemTarget::Existing { broadcast_port, .. } = &mut p.modem {
        *broadcast_port = Some(bcast);
    }
    station(&mut p);
    let id = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    assert!(lab.wait(30.0, |s| s.modem == ModemState::Listening).await.0, "{call} did not attach");
    let config = RigConfig { offset_hz, ..rig(rig_port) };
    h.call(AppCommand::SetRigConfig { config }).await.unwrap();
    assert!(lab.rig_up().await, "{call}: the rig link did not come up");
    Side { lab, rx }
}

fn qsy_stage(s: &AppSnapshot) -> Option<String> {
    s.qsy.as_ref().map(|q| q.stage.clone())
}

fn is_notice(e: &AppEvent, key: &str) -> bool {
    matches!(e, AppEvent::QsyNotice { key: k, .. } if k == key)
}

async fn m3c(out: &std::path::Path, data: &std::path::Path) -> Lab {
    let mut a = start_side(out, data, "ZS6LAB", 8300, 10000, 8100, 4532, 0, |p| {
        p.name = Some("Lab A".into());
        p.qth = Some("Pretoria".into());
        p.grid = Some("KG44dg".into());
    })
    .await;
    let mut b = start_side(out, data, "ZS6PEER", 8400, 10100, 8200, 4632, B_OFFSET, |p| {
        p.name = Some("Pat".into());
        p.qth = Some("Cape Town".into());
        p.grid = Some("JF96fc".into());
        p.email = Some("zs6peer@example.com".into());
    })
    .await;
    let (ha, hb) = (a.lab.h.clone(), b.lab.h.clone());

    // ---- 5: kind 2 (B's extended beacon) and kind 3 (A's suggestion) cross the broadcast plane.
    let fields = hb.call(AppCommand::SetBeaconFields { name: true, qth: true, status: Some("QRV 40m".into()) }).await;
    let sent2 = hb.call(AppCommand::SendBeaconNow).await;
    let end = Instant::now() + Duration::from_secs(60);
    let mut seen = None;
    while Instant::now() < end {
        let h = a.heard().await.into_iter().find(|s| s.callsign == "ZS6PEER");
        if h.as_ref().is_some_and(|s| s.status.is_some()) {
            seen = h;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let v = |r: &Option<Reading<String>>| r.as_ref().map(|r| r.value.clone());
    let kind2 = seen.as_ref().is_some_and(|s| {
        v(&s.name).as_deref() == Some("Pat")
            && v(&s.qth).as_deref() == Some("Cape Town")
            && v(&s.status).as_deref() == Some("QRV 40m")
    });
    let k2 = seen.map(|s| (v(&s.name), v(&s.qth), v(&s.status), s.grid));
    b.lab.idle().await;
    a.lab.idle().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let sug = a.entry("40m suggest", 7_110_000).await;
    b.drain();
    let sent3 = ha.call(AppCommand::SuggestQsy { to: "ZS6PEER".into(), entry_ids: vec![sug] }).await;
    let got = b.event(60.0, |e| matches!(e, AppEvent::QsySuggested { .. })).await;
    let kind3 = matches!(&got, Some(AppEvent::QsySuggested { from, candidates })
        if from == "ZS6LAB" && candidates.first().is_some_and(|c| c.freq_hz == 7_110_000 && c.mode == "USB"));
    a.lab.record(
        "5 kind 2 and kind 3",
        fields.is_ok() && sent2.is_ok() && kind2 && sent3.is_ok() && kind3,
        format!("fields {fields:?}, beacon {sent2:?}; A heard ZS6PEER (name, qth, status, grid) {k2:?}; suggestion {sent3:?} -> B {got:?}"),
    );
    a.lab.idle().await;
    b.lab.idle().await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    // ---- 1: QSY end to end. Each rig goes to the candidate minus its own offset; A calls again.
    ha.call(AppCommand::Call { peer: "ZS6PEER".into() }).await.unwrap();
    let (up, _) = a.lab.wait(120.0, |s| s.modem == ModemState::Connected).await;
    b.lab.wait(20.0, |s| s.session.is_some()).await;
    let first = a.session_id().await;
    let caps = a.lab.snap().await.session.map(|s| s.caps).unwrap_or_default();
    let new_hz = 7_120_000;
    let e1 = a.entry("40m qsy", new_hz).await;
    let p1 = ha.call(AppCommand::ProposeQsy { entry_ids: vec![e1], note: Some("lab".into()) }).await;
    let (offered, _) = b.lab.wait(60.0, |s| qsy_stage(s).as_deref() == Some("offered")).await;
    let ans1 = hb.call(AppCommand::AnswerQsy { accept: Some(0) }).await;
    let (a_moved, a_dial) = dial_is(4532, new_hz, 90.0).await;
    let (b_moved, b_dial) = dial_is(4632, new_hz - B_OFFSET as u64, 90.0).await;
    let (recalled, s) = a
        .lab
        .wait(150.0, |s| {
            s.modem == ModemState::Connected
                && s.session.as_ref().is_some_and(|x| Some(&x.session_id) != first.as_ref())
        })
        .await;
    let notes = a.lab.wait(10.0, |s| s.current_qso.as_ref().is_some_and(|q| q.notes.is_some())).await.1.current_qso;
    a.lab.wait(20.0, |s| s.qsy.is_none()).await;
    b.lab.wait(30.0, |s| s.qsy.is_none()).await;
    a.lab.record(
        "1 QSY end to end",
        up && caps.iter().any(|c| c == "qsy/1") && p1.is_ok() && offered && ans1.is_ok() && a_moved && b_moved && recalled,
        format!(
            "session {up} caps {caps:?}; propose {p1:?}; B offered {offered}; accept {ans1:?}; A dial {a_dial:?} (want {new_hz}), B dial {b_dial:?} (want {}); re-call CONNECTED {recalled} ({:?} -> {:?}); QSO notes {:?}",
            new_hz - B_OFFSET as u64,
            first,
            s.session.as_ref().map(|x| &x.session_id),
            notes.and_then(|q| q.notes)
        ),
    );

    // ---- 4: SNR and station info arrive from B; its email is not in them.
    a.drain();
    let r_snr = ha.call(AppCommand::RequestInfo { what: InfoWhat::Snr }).await;
    let snr = a
        .event(60.0, |e| {
            matches!(e, AppEvent::PeerInfo { snr: Some(_), .. } | AppEvent::PeerInfo { reason: Some(_), .. })
        })
        .await;
    let r_st = ha.call(AppCommand::RequestInfo { what: InfoWhat::Station }).await;
    let st = a
        .event(60.0, |e| {
            matches!(e, AppEvent::PeerInfo { station: Some(_), .. } | AppEvent::PeerInfo { reason: Some(_), .. })
        })
        .await;
    let snr_ok = matches!(&snr, Some(AppEvent::PeerInfo { snr: Some(x), .. }) if x.db.is_finite());
    let st_ok = matches!(&st, Some(AppEvent::PeerInfo { station: Some(x), .. })
        if x.name.as_deref() == Some("Pat") && x.qth.as_deref() == Some("Cape Town") && x.email.is_none());
    a.lab.record(
        "4 info",
        r_snr.is_ok() && snr_ok && r_st.is_ok() && st_ok,
        format!("snr {r_snr:?} -> {snr:?}; station {r_st:?} -> {st:?}"),
    );

    // ---- 3: A's gate fails between the proposal and the accept: A declines `rig`, the session goes on.
    // rigctld without -P keeps the link up with PTT unknown; a stopped rigctld would be the
    // no-rig (manual) path, which accepts (design B3).
    let session = a.session_id().await;
    let e3 = a.entry("40m gate", 7_130_000).await;
    a.drain();
    b.drain();
    let p3 = ha.call(AppCommand::ProposeQsy { entry_ids: vec![e3], note: None }).await;
    let (offered3, _) = b.lab.wait(60.0, |s| qsy_stage(s).as_deref() == Some("offered")).await;
    // Restart A's rigctld only between transmissions (M4): killing it mid-frame cuts A's
    // frame off on the air, and 1.9.25's ARQ then stalled ~40 s, past the answer window.
    let quiet_by = std::time::Instant::now() + Duration::from_secs(30);
    let mut off_for = 0u32;
    while off_for < 6 && std::time::Instant::now() < quiet_by {
        off_for = if a.lab.snap().await.rig.ptt == RigPttView::Off { off_for + 1 } else { 0 };
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    shell(&env("RIG_KILL"));
    shell(&env("RIG_START_NOP"));
    // A restarted dummy rig is back on Hamlib's default dial: put it where check 1 left it.
    shell(&format!("rigctl -m 2 -r 127.0.0.1:4532 F {new_hz} M USB 2400"));
    let (unknown, rs) = a.lab.wait(20.0, |s| s.rig.link == RigLinkView::Up && s.rig.ptt == RigPttView::Unknown).await;
    let ans3 = hb.call(AppCommand::AnswerQsy { accept: Some(0) }).await;
    let own = a.event(60.0, |e| is_notice(e, "qsy.notify.ownRig")).await.is_some();
    let told = b.event(60.0, |e| is_notice(e, "qsy.notify.declinedRig")).await.is_some();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let (sa, sb) = (a.lab.snap().await, b.lab.snap().await);
    let still = sa.session.as_ref().map(|s| &s.session_id) == session.as_ref() && sa.modem == ModemState::Connected;
    let (da, db) = (dial(4532), dial(4632));
    a.lab.record(
        "3 gate",
        p3.is_ok() && offered3 && unknown && ans3.is_ok() && own && told && still && sa.qsy.is_none() && sb.qsy.is_none()
            && da == Some(new_hz) && db == Some(new_hz - B_OFFSET as u64),
        format!(
            "propose {p3:?}; B offered {offered3}; A link {:?} PTT {:?}; accept {ans3:?}; A noted own rig {own}; B told declined (rig) {told}; same session {still}; dials A {da:?} B {db:?}",
            rs.rig.link, rs.rig.ptt
        ),
    );
    shell(&env("RIG_KILL"));
    shell(&env("RIG_START"));
    shell(&format!("rigctl -m 2 -r 127.0.0.1:4532 F {new_hz} M USB 2400"));
    a.lab.rig_up().await;

    // ---- 2: fallback. B stops answering calls, so A's re-calls go unanswered; both rigs return.
    let back_a = dial(4532).unwrap_or(new_hz);
    let back_b = dial(4632).unwrap_or(new_hz - B_OFFSET as u64);
    let ro = hb.call(AppCommand::SetReceiveOnly { on: true }).await;
    let fb_hz = 7_140_000;
    let e2 = a.entry("40m fallback", fb_hz).await;
    a.drain();
    b.drain();
    let p2 = ha.call(AppCommand::ProposeQsy { entry_ids: vec![e2], note: None }).await;
    let (offered2, _) = b.lab.wait(60.0, |s| qsy_stage(s).as_deref() == Some("offered")).await;
    let ans2 = hb.call(AppCommand::AnswerQsy { accept: Some(0) }).await;
    let (a_went, _) = dial_is(4532, fb_hz, 90.0).await;
    let (b_went, _) = dial_is(4632, fb_hz - B_OFFSET as u64, 90.0).await;
    let t0 = Instant::now();
    let (a_back, a_d) = dial_is(4532, back_a, 200.0).await;
    let a_took = t0.elapsed().as_secs_f32();
    let (b_back, b_d) = dial_is(4632, back_b, 60.0).await;
    let b_took = t0.elapsed().as_secs_f32();
    let fell = a.event(5.0, |e| is_notice(e, "qsy.notify.fellBack")).await.is_some()
        || a.lab.activity().await.iter().any(|e| e.text.to_lowercase().contains("original"));
    let (a_over, sa) = a.lab.wait(30.0, |s| s.qsy.is_none()).await;
    let (b_over, _) = b.lab.wait(30.0, |s| s.qsy.is_none()).await;
    a.lab.record(
        "2 fallback",
        ro.is_ok() && p2.is_ok() && offered2 && ans2.is_ok() && a_went && b_went && a_back && b_back && a_over && b_over
            && sa.session.is_none(),
        format!(
            "receive-only {ro:?}; propose {p2:?}; offered {offered2}; accept {ans2:?}; both moved A {a_went} B {b_went}; back A {a_back} {a_d:?} after {a_took:.0} s, B {b_back} {b_d:?} after {b_took:.0} s (want {back_a}/{back_b}); fell-back note {fell}; QSY over A {a_over} B {b_over}; A modem {:?}",
            sa.modem
        ),
    );
    let _ = hb.call(AppCommand::SetReceiveOnly { on: false }).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), hb.call(AppCommand::Shutdown)).await;
    let log: Vec<String> =
        b.lab.activity().await.iter().map(|e| format!("{} [{}] {}", e.at_utc, e.kind, e.text)).collect();
    std::fs::write(out.join("activity-m3c-b.txt"), log.join("\n") + "\n").unwrap();
    a.lab
}
