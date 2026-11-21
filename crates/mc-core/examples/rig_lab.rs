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

use mc_core::app::{
    spawn_app, ActivityEntry, AppCommand, AppHandle, AppOptions, AppSnapshot, AudioSettingsView, BandLevels, CommandOk,
    FreqControl, ModemState, OwnerCheck, PttMethod, RigBanner, RigChange, RigConfig, RigLinkView, RigPttView,
    TxLevelResult,
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
        other => panic!("unknown mode {other}: m3b, m3b1 or reuse"),
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
