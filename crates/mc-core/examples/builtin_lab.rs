//! M5 built-in lab (lab/docker/rig/run-builtin-lab.sh, M5-checks.md): the real
//! service actor, headless, running its OWN Mercury and rigctld from the
//! bundle (`MC_BUNDLE_DIR`): Hamlib's dummy rig, crossed PulseAudio null sinks,
//! no transceiver, no serial port, no RF.
//!
//!   Station A: ZS6BLA, control 8300/8301, broadcast 8100, UI 10000, rigctld 4532
//!   Station B: ZS6BLB, control 8400/8401, broadcast 8200, UI 10100, rigctld 4632
//!
//! `builtin_lab hold <out>`: station A alone; prints `HOLD READY` once it
//! listens with its rigctld running, then waits to be killed (check 3 is the
//! script's: nothing of ours may be left behind).
//! `builtin_lab pair <out>`: checks 1, 2 and 4; one `PASS|FAIL <check>: <detail>`
//! line each, and a non-zero exit on any FAIL.

use mc_core::app::files::{TransferDirection, TransferState};
use mc_core::app::{
    spawn_app, AppCommand, AppHandle, AppOptions, AppSnapshot, CommandOk, FreqControl, ModemDetail, ModemState,
    PttMethod, RigConfig,
};
use mc_core::bundle::Bundle;
use mc_core::files::PhotoSize;
use mc_core::profile::{BuiltInPorts, ModemTarget, StationProfile};
use mc_storage::Store;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::Instant;

const A: (&str, BuiltInPorts) = ("ZS6BLA", BuiltInPorts { control: 8300, broadcast: 8100, ui: 10000, rigctld: 4532 });
const B: (&str, BuiltInPorts) = ("ZS6BLB", BuiltInPorts { control: 8400, broadcast: 8200, ui: 10100, rigctld: 4632 });

fn managed_dummy() -> RigConfig {
    let mut c = RigConfig { freq_control: FreqControl::ManagedRigctld, ..RigConfig::default() };
    c.cat.model = Some(1); // Hamlib's dummy rig: no serial port, no radio
    c.cat.serial_port = "/dev/null".into();
    c.ptt.method = PttMethod::Cat;
    c
}

struct Side {
    name: &'static str,
    h: AppHandle,
    id: String,
}

impl Side {
    async fn snap(&self) -> AppSnapshot {
        match self.h.call(AppCommand::Snapshot).await.unwrap() {
            CommandOk::Snapshot(s) => s,
            other => panic!("{other:?}"),
        }
    }

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
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// A new app with a built-in profile, selected; then its managed rigctld
    /// set and the profile selected again, so rigctld starts before Mercury.
    async fn start(root: &Path, bundle: &Bundle, call: &'static str, ports: BuiltInPorts) -> Side {
        let o = AppOptions {
            stations_root: Some(root.join(call).join("stations")),
            bundle: Some(bundle.clone()),
            ..AppOptions::default()
        };
        let h = spawn_app(Store::open_in_memory().unwrap(), o);
        let mut p = StationProfile::new_builtin(call);
        p.modem = ModemTarget::BuiltIn { ports };
        let id = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
            CommandOk::Profile(p) => p.id,
            other => panic!("{other:?}"),
        };
        let side = Side { name: call, h, id };
        side.select().await;
        side.wait(30.0, |s| s.modem == ModemState::Listening).await;
        side.h.call(AppCommand::SetRigConfig { config: managed_dummy() }).await.unwrap();
        side.h.call(AppCommand::SelectProfile { id: None }).await.unwrap();
        side.select().await;
        side
    }

    async fn select(&self) {
        self.h.call(AppCommand::SelectProfile { id: Some(self.id.clone()) }).await.unwrap();
    }

    fn ready(s: &AppSnapshot) -> bool {
        s.modem == ModemState::Listening && s.rig.daemon.as_deref() == Some("running")
    }

    /// Crossed null sinks: this side plays into `out` and hears `input`.
    async fn audio(&self, input: &str, output: &str) -> bool {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            let cmd = AppCommand::SetAudio {
                system: "pulse".into(),
                input: input.into(),
                output: output.into(),
                channel: "left".into(),
            };
            if self.h.call(cmd).await.is_ok() {
                break;
            }
            if Instant::now() >= end {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        self.wait(20.0, |s| s.builtin_detail.is_none()).await.0
    }
}

fn line(results: &mut Vec<bool>, check: &str, ok: bool, detail: String) {
    println!("{} {check}: {detail}", if ok { "PASS" } else { "FAIL" });
    results.push(ok);
}

/// Every listening socket of a Mercury or rigctld process is on 127.0.0.1
/// (read from /proc: the lab image has no `ss`).
fn listeners_loopback_only() -> (bool, String) {
    let mut inodes = std::collections::HashSet::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
        if !matches!(comm.trim(), "mercury" | "rigctld") {
            continue;
        }
        for fd in std::fs::read_dir(e.path().join("fd")).into_iter().flatten().flatten() {
            if let Ok(link) = std::fs::read_link(fd.path()) {
                let l = link.to_string_lossy().to_string();
                if let Some(n) = l.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')) {
                    inodes.insert(n.to_string());
                }
            }
        }
    }
    let (mut ours, mut bad) = (0, Vec::new());
    for (file, loopback) in [("/proc/net/tcp", "0100007F"), ("/proc/net/tcp6", "00000000000000000000000001000000")] {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for l in text.lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            // local_address, state (0A: listening) and inode
            if f.len() < 10 || f[3] != "0A" || !inodes.contains(f[9]) {
                continue;
            }
            ours += 1;
            let addr = f[1].split(':').next().unwrap_or("");
            if addr != loopback {
                bad.push(format!("{file} {}", f[1]));
            }
        }
    }
    (ours > 0 && bad.is_empty(), format!("{ours} listeners of mercury/rigctld, not on loopback: {bad:?}"))
}

async fn pair(out: &Path, bundle: Bundle) -> bool {
    let mut results = Vec::new();
    let a = Side::start(out, &bundle, A.0, A.1).await;
    let b = Side::start(out, &bundle, B.0, B.1).await;
    let (ra, sa) = a.wait(60.0, Side::ready).await;
    let (rb, sb) = b.wait(60.0, Side::ready).await;
    let aud_a = a.audio("b_to_a.monitor", "a_to_b").await;
    let aud_b = b.audio("a_to_b.monitor", "b_to_a").await;
    let (lo, lo_detail) = listeners_loopback_only();
    line(
        &mut results,
        "1 both READY, loopback only",
        ra && rb && aud_a && aud_b && lo,
        format!(
            "A {:?}/{:?} rigctld {:?}, B {:?}/{:?} rigctld {:?}; audio set A {aud_a} B {aud_b}; {lo_detail}",
            sa.modem, sa.builtin_detail, sa.rig.daemon, sb.modem, sb.builtin_detail, sb.rig.daemon
        ),
    );

    // ---- 2: a session and a 4 KiB file.
    let file = out.join("lab-4k.bin");
    let bytes: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    std::fs::write(&file, &bytes).unwrap();
    a.h.call(AppCommand::Call { peer: B.0.into() }).await.unwrap();
    let (up, _) = a.wait(180.0, |s| s.modem == ModemState::Connected && s.session.is_some()).await;
    let (b_up, _) = b.wait(30.0, |s| s.session.is_some()).await;
    let prep = match a
        .h
        .call(AppCommand::PrepareFile { path: file.display().to_string(), photo: PhotoSize::Original })
        .await
    {
        Ok(CommandOk::Prepared(p)) => Some(p.prep_id),
        other => {
            eprintln!("prepare: {other:?}");
            None
        }
    };
    let sent = match &prep {
        Some(id) => a.h.call(AppCommand::SendFile { prep_id: id.clone() }).await.is_ok(),
        None => false,
    };
    let transfers = |h: &AppHandle| {
        let h = h.clone();
        async move {
            match h.call(AppCommand::Transfers).await {
                Ok(CommandOk::Transfers { transfers }) => transfers,
                _ => Vec::new(),
            }
        }
    };
    let mut accepted = false;
    let mut arrived = None;
    let end = Instant::now() + Duration::from_secs(300);
    while Instant::now() < end {
        for t in transfers(&b.h).await.into_iter().filter(|t| t.direction == TransferDirection::In) {
            match t.state {
                TransferState::Offered if !accepted => {
                    accepted = b.h.call(AppCommand::AcceptFile { id: t.id.clone() }).await.is_ok();
                }
                TransferState::Complete => arrived = Some(t),
                _ => {}
            }
        }
        if arrived.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let saved = arrived.as_ref().and_then(|t| t.saved_name.clone().or(Some(t.original_name.clone())));
    let same = saved
        .as_ref()
        .and_then(|n| {
            walk(&out.join(B.0)).into_iter().find(|p| p.file_name().is_some_and(|f| f.to_string_lossy() == *n))
        })
        .and_then(|p| std::fs::read(p).ok())
        .is_some_and(|got| got == bytes);
    line(
        &mut results,
        "2 a session and a 4 KiB file",
        up && b_up && sent && arrived.is_some() && same,
        format!("session A {up} B {b_up}; prepared {prep:?} sent {sent}; B accepted {accepted}; arrived {:?}; byte-exact {same}", arrived.map(|t| (t.state, t.size))),
    );
    let _ = a.h.call(AppCommand::Disconnect).await;
    a.wait(60.0, |s| s.session.is_none()).await;

    // ---- 4: a port held by another process is named, never adopted.
    let held = std::net::TcpListener::bind(("127.0.0.1", 8700)).expect("hold 8700");
    let mut p = StationProfile::new_builtin("ZS6BLC");
    p.modem = ModemTarget::BuiltIn { ports: BuiltInPorts { control: 8700, broadcast: 8710, ui: 10300, rigctld: 4732 } };
    let id = match a.h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    a.h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    let (refused, s) = a
        .wait(
            30.0,
            |s| matches!(&s.builtin_detail, Some(ModemDetail::MercuryStopped { reason }) if reason.contains("8700")),
        )
        .await;
    held.set_nonblocking(true).unwrap();
    let nobody_connected = matches!(held.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock);
    line(
        &mut results,
        "4 a taken port is named, not adopted",
        refused && s.modem == ModemState::Faulted && nobody_connected,
        format!(
            "{:?} / {:?} / {:?}; nothing connected to the holder {nobody_connected}",
            s.modem, s.modem_detail, s.builtin_detail
        ),
    );

    for side in [&a, &b] {
        let _ = side.h.call(AppCommand::Shutdown).await;
        let _ = side.name;
    }
    results.iter().all(|ok| *ok)
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (mode, out) = (args.get(1).cloned().unwrap_or_default(), PathBuf::from(args.get(2).expect("<mode> <out>")));
    let bundle = Bundle::from_dir(PathBuf::from(std::env::var("MC_BUNDLE_DIR").expect("MC_BUNDLE_DIR")))
        .expect("a bundle with MANIFEST.sha256");
    std::fs::create_dir_all(&out).unwrap();
    match mode.as_str() {
        "hold" => {
            let a = Side::start(&out, &bundle, A.0, A.1).await;
            let (ok, s) = a.wait(60.0, Side::ready).await;
            if !ok {
                println!("HOLD FAILED {:?} {:?} {:?}", s.modem, s.builtin_detail, s.rig.daemon);
                std::process::exit(1);
            }
            println!("HOLD READY");
            std::future::pending::<()>().await;
        }
        "pair" => {
            if !pair(&out, bundle).await {
                std::process::exit(1);
            }
        }
        other => panic!("mode {other:?}: hold or pair"),
    }
}
