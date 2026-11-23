//! M5 Tasks 3–5: the built-in modem (ports, the supervisor, the service).

use mc_core::app::builtin::ports::{check_free, PortTaken};
use mc_core::profile::BuiltInPorts;
use std::net::TcpListener;

/// M5 §5.1: a taken port is named, with a set that is free throughout.
#[test]
fn taken_port_is_named_with_a_free_set() {
    // Ports well away from other tests' defaults.
    let base = BuiltInPorts { control: 18300, broadcast: 18100, ui: 20000, rigctld: 14532 };
    let hold = TcpListener::bind(("127.0.0.1", base.broadcast)).unwrap();
    let PortTaken { port, suggestion } = check_free(&base).unwrap_err();
    assert_eq!(port, base.broadcast);
    assert_ne!(suggestion, base);
    for (_, p) in suggestion.all() {
        assert!(TcpListener::bind(("127.0.0.1", p)).is_ok(), "suggested port {p} is free");
    }
    drop(hold);
    assert!(check_free(&base).is_ok());
}

// ---- Task 4: the supervisor (M5 §5.3–5.4) ------------------------------------

use mc_core::app::builtin::ini::write_ini;
use mc_core::app::builtin::{BuiltInStart, MercuryPtt, StartError, Supervisor};
use mc_core::app::PttLine;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// mercury-adapter's fake-mercury program, built once per test run (another
/// package's binary, so Cargo does not hand its path to these tests).
fn fake_mercury() -> PathBuf {
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let exe = if cfg!(windows) { "fake-mercury.exe" } else { "fake-mercury" };
            let me = std::env::current_exe().unwrap();
            let dir = me
                .ancestors()
                .find(|a| a.join(".cargo-build-lock").is_file() || a.join("deps").is_dir())
                .expect("target profile folder")
                .to_path_buf();
            let ok = std::process::Command::new(env!("CARGO"))
                .args(["build", "-p", "mercury-adapter", "--bin", "fake-mercury", "--bin", "fake-rigctld"])
                .status()
                .unwrap()
                .success();
            assert!(ok, "build fake-mercury");
            let p = dir.join(exe);
            assert!(p.is_file(), "{}", p.display());
            p
        })
        .clone()
}

/// A free set of ports, distinct for every test in this run.
fn fresh_ports() -> BuiltInPorts {
    static NEXT: AtomicU16 = AtomicU16::new(0);
    loop {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let p =
            BuiltInPorts { control: 21000 + n * 10, broadcast: 23000 + n * 10, ui: 25000 + n * 10, rigctld: 27000 + n };
        if check_free(&p).is_ok() {
            return p;
        }
    }
}

fn start_cfg(dir: &Path, ports: BuiltInPorts) -> BuiltInStart {
    BuiltInStart { mercury: fake_mercury(), dir: dir.to_path_buf(), ports, ptt: MercuryPtt::None, audio_null: true }
}

/// The control port answers `VERSION`.
async fn answers(port: u16) -> bool {
    let Ok(Ok(mut s)) =
        tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(("127.0.0.1", port))).await
    else {
        return false;
    };
    if s.write_all(b"VERSION\r").await.is_err() {
        return false;
    }
    let (r, _w) = s.split();
    let mut r = BufReader::new(r);
    // Mercury greets on connect; the reply to VERSION follows.
    let read = async {
        loop {
            let mut line = Vec::new();
            match r.read_until(b'\r', &mut line).await {
                Ok(0) | Err(_) => return false,
                Ok(_) if line.trim_ascii_start().starts_with(b"VERSION") => return true,
                Ok(_) => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(2), read).await.unwrap_or(false)
}

async fn closed_within(port: u16, limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

fn ini_text(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("mercury.ini")).unwrap()
}

/// The value of `key` in `[section]`, as written.
fn ini_get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut sec = String::new();
    for l in text.lines().map(str::trim) {
        if let Some(s) = l.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            sec = s.to_string();
        } else if let Some((k, v)) = l.split_once('=') {
            if sec == section && k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// M5 §5.3: loopback only, no web origins, our ports, no store; `.bak` kept.
#[test]
fn ini_has_loopback_and_no_origins() {
    let d = tempfile::tempdir().unwrap();
    let ports = BuiltInPorts { control: 8300, broadcast: 8100, ui: 10000, rigctld: 4532 };
    let path = write_ini(d.path(), &ports, &MercuryPtt::Rigctld { port: 4532 }).unwrap();
    assert_eq!(path, d.path().join("mercury.ini"));
    let t = ini_text(d.path());
    assert_eq!(ini_get(&t, "main", "bind_address").as_deref(), Some("127.0.0.1"));
    assert_eq!(ini_get(&t, "main", "ui_allowed_origins").as_deref(), Some("\"\""));
    assert_eq!(ini_get(&t, "main", "ui_enabled").as_deref(), Some("true"));
    assert_eq!(ini_get(&t, "main", "ui_port").as_deref(), Some("10000"));
    assert_eq!(ini_get(&t, "main", "arq_tcp_base_port").as_deref(), Some("8300"));
    assert_eq!(ini_get(&t, "main", "broadcast_tcp_port").as_deref(), Some("8100"));
    assert_eq!(ini_get(&t, "store", "enabled").as_deref(), Some("false"));
    assert_eq!(ini_get(&t, "ptt", "method").as_deref(), Some("hamlib"));
    assert_eq!(ini_get(&t, "ptt", "hamlib_model").as_deref(), Some("2"));
    assert_eq!(ini_get(&t, "ptt", "device").as_deref(), Some("\"127.0.0.1:4532\""));
    assert!(!d.path().join("mercury.ini.bak").exists(), "nothing to back up the first time");

    let moved = BuiltInPorts { control: 8310, ..ports };
    write_ini(d.path(), &moved, &MercuryPtt::Serial { port: "COM7".into(), line: PttLine::Dtr }).unwrap();
    let bak = std::fs::read_to_string(d.path().join("mercury.ini.bak")).unwrap();
    assert_eq!(bak, t, "the previous ini is kept as mercury.ini.bak");
    let t2 = ini_text(d.path());
    assert_eq!(ini_get(&t2, "main", "arq_tcp_base_port").as_deref(), Some("8310"));
    assert_eq!(ini_get(&t2, "ptt", "method").as_deref(), Some("serial"));
    assert_eq!(ini_get(&t2, "ptt", "device").as_deref(), Some("\"COM7\""));
    assert_eq!(ini_get(&t2, "ptt", "line").as_deref(), Some("dtr"));

    write_ini(d.path(), &moved, &MercuryPtt::Cm108 { device: "/dev/hidraw0".into() }).unwrap();
    let t3 = ini_text(d.path());
    assert_eq!(ini_get(&t3, "ptt", "method").as_deref(), Some("cm108"));
    assert_eq!(ini_get(&t3, "ptt", "device").as_deref(), Some("\"/dev/hidraw0\""));
    write_ini(d.path(), &moved, &MercuryPtt::None).unwrap();
    assert_eq!(ini_get(&ini_text(d.path()), "ptt", "method").as_deref(), Some("none"));
}

/// M5 §5.3: Mercury saves the sound card into this ini; MercuryChat keeps it.
#[test]
fn ini_keeps_mercury_saved_audio() {
    let d = tempfile::tempdir().unwrap();
    let ports = BuiltInPorts::default();
    write_ini(d.path(), &ports, &MercuryPtt::None).unwrap();
    // What Mercury's cfg_write produces once the operator picks a card,
    // including a widened ui_allowed_origins MercuryChat must put back.
    std::fs::write(
        d.path().join("mercury.ini"),
        "[main]\nui_enabled = true\nui_port = 10000\ninput_device = \"hw:1,0\"\noutput_device = \"hw:1,0\"\n\
         capture_channel = left\nsound_system = alsa\nbind_address = 0.0.0.0\nui_allowed_origins = \"*\"\n\
         \n[ptt]\nmethod = none\n\n[audio]\ntx_gain_db = -3.00\ntx_delay_ms = 40\n\n[store]\nenabled = true\n",
    )
    .unwrap();
    write_ini(d.path(), &ports, &MercuryPtt::None).unwrap();
    let t = ini_text(d.path());
    assert_eq!(ini_get(&t, "main", "input_device").as_deref(), Some("\"hw:1,0\""));
    assert_eq!(ini_get(&t, "main", "output_device").as_deref(), Some("\"hw:1,0\""));
    assert_eq!(ini_get(&t, "main", "capture_channel").as_deref(), Some("left"));
    assert_eq!(ini_get(&t, "main", "sound_system").as_deref(), Some("alsa"));
    assert_eq!(ini_get(&t, "audio", "tx_gain_db").as_deref(), Some("-3.00"));
    assert_eq!(ini_get(&t, "audio", "tx_delay_ms").as_deref(), Some("40"));
    assert_eq!(ini_get(&t, "main", "bind_address").as_deref(), Some("127.0.0.1"));
    assert_eq!(ini_get(&t, "main", "ui_allowed_origins").as_deref(), Some("\"\""));
    assert_eq!(ini_get(&t, "store", "enabled").as_deref(), Some("false"));
    assert_eq!(t.matches("bind_address").count(), 1, "one bind_address only:\n{t}");
}

#[tokio::test]
async fn starts_and_answers() {
    let d = tempfile::tempdir().unwrap();
    let ports = fresh_ports();
    let sup = Supervisor::start(start_cfg(d.path(), ports)).await.unwrap();
    assert!(answers(ports.control).await);
    let log = std::fs::read_to_string(d.path().join("mercury.log")).unwrap();
    assert!(log.contains("-x null"), "audio null on first start: {log}");
    assert!(log.contains(&format!("-C {}", d.path().join("mercury.ini").display())), "{log}");
    sup.stop().await;
    assert!(closed_within(ports.control, Duration::from_secs(6)).await);
}

/// M5 §5.4: MercuryChat stops only the process it started.
#[tokio::test]
async fn stop_ends_only_our_process() {
    let other_dir = tempfile::tempdir().unwrap();
    let other = fresh_ports();
    let mut theirs = std::process::Command::new(fake_mercury())
        .args(["-p", &other.control.to_string(), "-b", &other.broadcast.to_string()])
        .args(["-U", &other.ui.to_string(), "-L"])
        .arg(other_dir.path().join("mercury.log"))
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while !answers(other.control).await {
        assert!(Instant::now() < end, "the other fake-mercury answers");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let d = tempfile::tempdir().unwrap();
    let ours = fresh_ports();
    let sup = Supervisor::start(start_cfg(d.path(), ours)).await.unwrap();
    sup.stop().await;
    assert!(closed_within(ours.control, Duration::from_secs(6)).await);
    assert!(answers(other.control).await, "someone else's Mercury is left running");
    assert!(theirs.try_wait().unwrap().is_none());
    theirs.kill().unwrap();
    let _ = theirs.wait();
}

/// M5 §5.1: a Mercury (or anything) already on the ports is never adopted.
#[tokio::test]
async fn never_adopts_taken_port() {
    let d = tempfile::tempdir().unwrap();
    let ports = fresh_ports();
    let hold = TcpListener::bind(("127.0.0.1", ports.ui)).unwrap();
    match Supervisor::start(start_cfg(d.path(), ports)).await {
        Err(StartError::PortTaken(PortTaken { port, .. })) => assert_eq!(port, ports.ui),
        Err(e) => panic!("expected PortTaken, got {e:?}"),
        Ok(_) => panic!("expected PortTaken, but it started"),
    }
    assert!(!d.path().join("mercury.log").exists(), "nothing was started");
    assert!(!answers(ports.control).await);
    drop(hold);
}

/// The process `children_die_with_the_app` kills: starts a Supervisor and waits.
#[tokio::test]
#[ignore = "run by children_die_with_the_app as a child process"]
async fn holder_process() {
    let dir = PathBuf::from(std::env::var("MC_HOLDER_DIR").unwrap());
    let ports = BuiltInPorts {
        control: std::env::var("MC_HOLDER_CONTROL").unwrap().parse().unwrap(),
        broadcast: std::env::var("MC_HOLDER_BCAST").unwrap().parse().unwrap(),
        ui: std::env::var("MC_HOLDER_UI").unwrap().parse().unwrap(),
        rigctld: 4532,
    };
    let mut cfg = start_cfg(&dir, ports);
    cfg.mercury = PathBuf::from(std::env::var("MC_HOLDER_MERCURY").unwrap());
    let _sup = Supervisor::start(cfg).await.unwrap();
    println!("HOLDER READY");
    std::future::pending::<()>().await;
}

/// M5 §5.4: when the app dies hard, its Mercury goes too (job object / pdeathsig).
#[tokio::test]
async fn children_die_with_the_app() {
    let d = tempfile::tempdir().unwrap();
    let ports = fresh_ports();
    let mut holder = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["holder_process", "--exact", "--ignored", "--nocapture"])
        .env("MC_HOLDER_DIR", d.path())
        .env("MC_HOLDER_CONTROL", ports.control.to_string())
        .env("MC_HOLDER_BCAST", ports.broadcast.to_string())
        .env("MC_HOLDER_UI", ports.ui.to_string())
        .env("MC_HOLDER_MERCURY", fake_mercury())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(30);
    while !answers(ports.control).await {
        assert!(Instant::now() < end, "the holder's Mercury answers");
        assert!(holder.try_wait().unwrap().is_none(), "the holder is running");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    holder.kill().unwrap();
    let _ = holder.wait();
    assert!(closed_within(ports.control, Duration::from_secs(3)).await, "Mercury died with the app");
}

/// A data folder under a name with spaces and accents (Windows user names).
#[tokio::test]
async fn starts_from_a_folder_with_spaces_and_accents() {
    let root = tempfile::tempdir().unwrap();
    let d = root.path().join("José Smith").join("mercury");
    let ports = fresh_ports();
    let sup = Supervisor::start(start_cfg(&d, ports)).await.unwrap();
    assert!(answers(ports.control).await);
    assert!(d.join("mercury.ini").is_file());
    assert!(d.join("mercury.log").is_file());
    sup.stop().await;
}

/// M5 §5.4: the supervisor reports an exit with the log's tail; restarting
/// is the caller's rule (Task 5). fake-mercury reads extra flags from
/// `fake-mercury.args` beside its log.
#[tokio::test]
async fn mercury_restart_rule_is_the_callers() {
    let d = tempfile::tempdir().unwrap();
    let ports = fresh_ports();
    std::fs::write(d.path().join("fake-mercury.args"), "--die-after 1500").unwrap();
    let mut sup = Supervisor::start(start_cfg(d.path(), ports)).await.unwrap();
    let info = tokio::time::timeout(Duration::from_secs(10), sup.exited()).await.expect("exits");
    assert_eq!(info.code, Some(3));
    assert!(info.log_tail.contains("fake-mercury: dying"), "{}", info.log_tail);
    assert!(info.log_tail.lines().count() <= 40);
}

// ---- Task 5: the built-in modem in the service (M5 §5.4–5.5) -----------------

use mc_core::app::{
    spawn_app, AppCommand, AppHandle, AppOptions, AppSnapshot, AudioSettingsView, CatLink, CommandOk, FreqControl,
    ModemDetail, ModemState, PttMethod, RigConfig,
};
use mc_core::bundle::Bundle;
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// A bundle folder with fake-mercury as `mercury` and fake-rigctld as
/// `rigctld`, its manifest and BUILD.txt, as the installer lays it out.
fn test_bundle(root: &Path) -> Bundle {
    let dir = root.join("modem");
    std::fs::create_dir_all(&dir).unwrap();
    let built = fake_mercury();
    let mut manifest = String::new();
    for (from, to) in [("fake-mercury", "mercury"), ("fake-rigctld", "rigctld")] {
        let to = format!("{to}{EXE}");
        std::fs::copy(built.with_file_name(format!("{from}{EXE}")), dir.join(&to)).unwrap();
        let bytes = std::fs::read(dir.join(&to)).unwrap();
        use sha2::Digest;
        manifest.push_str(&format!("{}  {to}\n", hex::encode(sha2::Sha256::digest(&bytes))));
    }
    std::fs::write(dir.join("MANIFEST.sha256"), manifest).unwrap();
    std::fs::write(dir.join("BUILD.txt"), "hamlib_version=4.7.2\nmercury_version=v1.9.25\n").unwrap();
    Bundle::from_dir(dir).unwrap()
}

fn builtin_profile(ports: BuiltInPorts) -> StationProfile {
    let mut p = StationProfile::new_builtin("ZS6ABC");
    p.modem = ModemTarget::BuiltIn { ports };
    p
}

/// A managed rigctld on Hamlib's dummy rig, keying by CAT.
fn managed_dummy() -> RigConfig {
    let mut c = RigConfig { freq_control: FreqControl::ManagedRigctld, ..RigConfig::default() };
    c.cat.model = Some(1);
    c.cat.link = CatLink::Serial;
    c.cat.serial_port = "COM10".into();
    c.ptt.method = PttMethod::Cat;
    c
}

struct Station {
    h: AppHandle,
    id: String,
    mercury_dir: PathBuf,
    ports: BuiltInPorts,
}

async fn snap(h: &AppHandle) -> AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    }
}

async fn snap_until(h: &AppHandle, secs: u64, what: &str, pred: impl Fn(&AppSnapshot) -> bool) -> AppSnapshot {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let s = snap(h).await;
        if pred(&s) {
            return s;
        }
        assert!(
            Instant::now() < end,
            "timed out waiting for {what}: modem {:?} ({:?}), builtin {:?}, rig daemon {:?}",
            s.modem,
            s.modem_detail,
            s.builtin_detail,
            s.rig.daemon
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn listening(s: &AppSnapshot) -> bool {
    s.modem == ModemState::Listening
}

/// An app with a built-in profile saved (not yet selected). `args` go to
/// fake-mercury through `fake-mercury.args` in the profile's Mercury folder.
async fn station(root: &Path, bundle: Bundle, args: &str) -> Station {
    let o = AppOptions {
        stations_root: Some(root.join("stations")),
        logs_dir: Some(root.join("logs")),
        bundle: Some(bundle),
        ..AppOptions::default()
    };
    let h = spawn_app(Store::open_in_memory().unwrap(), o);
    let ports = fresh_ports();
    let id = match h.call(AppCommand::SaveProfile { profile: builtin_profile(ports) }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mercury_dir = root.join("stations").join(&id).join("mercury");
    std::fs::create_dir_all(&mercury_dir).unwrap();
    if !args.is_empty() {
        std::fs::write(mercury_dir.join("fake-mercury.args"), args).unwrap();
    }
    Station { h, id, mercury_dir, ports }
}

impl Station {
    async fn select(&self) {
        self.h.call(AppCommand::SelectProfile { id: Some(self.id.clone()) }).await.unwrap();
    }

    async fn deselect(&self) {
        self.h.call(AppCommand::SelectProfile { id: None }).await.unwrap();
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.mercury_dir.join("mercury.log")).unwrap_or_default()
    }

    /// fake-mercury's start lines, one per start.
    fn starts(&self) -> Vec<String> {
        self.log().lines().filter(|l| l.starts_with("fake-mercury: args")).map(String::from).collect()
    }

    async fn audio(&self) -> AudioSettingsView {
        match self.h.call(AppCommand::AudioSettings).await.unwrap() {
            CommandOk::Audio(v) => v,
            other => panic!("{other:?}"),
        }
    }
}

fn set_audio() -> AppCommand {
    AppCommand::SetAudio { system: "wasapi".into(), input: "in2".into(), output: "out2".into(), channel: "left".into() }
}

#[tokio::test(flavor = "multi_thread")]
async fn builtin_profile_reaches_ready() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    st.select().await;
    snap_until(&st.h, 20, "the built-in modem listening", listening).await;
    let ini = std::fs::read_to_string(st.mercury_dir.join("mercury.ini")).unwrap();
    assert_eq!(ini_get(&ini, "main", "bind_address").as_deref(), Some("127.0.0.1"));
    assert_eq!(st.starts().len(), 1);
}

/// M5 §5.4: rigctld first, answering, then Mercury.
#[tokio::test(flavor = "multi_thread")]
async fn rigctld_starts_before_mercury() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    st.select().await;
    snap_until(&st.h, 20, "listening", listening).await;
    assert_eq!(st.h.call(AppCommand::SetRigConfig { config: managed_dummy() }).await.unwrap(), CommandOk::Done);
    st.deselect().await;
    assert!(closed_within(st.ports.control, Duration::from_secs(6)).await);
    std::fs::write(st.mercury_dir.join("fake-mercury.args"), format!("--expect-rigctld {}", st.ports.rigctld)).unwrap();
    st.select().await;
    let s = snap_until(&st.h, 20, "listening with our rigctld", |s| {
        listening(s) && s.rig.daemon.as_deref() == Some("running")
    })
    .await;
    assert_eq!(s.rig.daemon.as_deref(), Some("running"));
    let log = st.log();
    let last = log.rsplit("fake-mercury: args").next().unwrap();
    assert!(last.contains("fake-mercury: rigctld answered"), "{log}");
}

/// M5 §5.4: a built-in profile never uses a rigctld it did not start.
#[tokio::test(flavor = "multi_thread")]
async fn builtin_never_reuses_a_running_rigctld() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    let theirs =
        mercury_adapter::fake_rig::FakeRigctld::start_at(format!("127.0.0.1:{}", st.ports.rigctld).parse().unwrap())
            .await;
    st.select().await;
    snap_until(&st.h, 20, "listening", listening).await;
    st.h.call(AppCommand::SetRigConfig { config: managed_dummy() }).await.unwrap();
    let s = snap_until(&st.h, 20, "the rigctld refused", |s| {
        s.rig.daemon.as_deref().is_some_and(|d| d.starts_with("failed"))
    })
    .await;
    let d = s.rig.daemon.unwrap();
    assert!(d.contains(&st.ports.rigctld.to_string()), "{d}");
    assert!(mercury_adapter::rigctl::answers_as_rigctld(theirs.addr).await, "theirs is left alone");
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_profile_stops_our_mercury() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    st.select().await;
    snap_until(&st.h, 20, "listening", listening).await;
    st.deselect().await;
    assert!(closed_within(st.ports.control, Duration::from_secs(6)).await, "our Mercury stopped");
    // The same ports again at once: the old Mercury has really gone.
    st.select().await;
    snap_until(&st.h, 20, "listening again", listening).await;
    assert_eq!(st.starts().len(), 2);
}

/// M5 §5.5 (LAB-01): no real sound device until the operator picks one.
#[tokio::test(flavor = "multi_thread")]
async fn first_start_is_silent_until_a_card_is_chosen() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    st.select().await;
    let s = snap_until(&st.h, 20, "listening", listening).await;
    assert_eq!(s.builtin_detail, Some(ModemDetail::ChooseSoundCard));
    assert!(st.starts()[0].contains("-x null"), "{:?}", st.starts());

    let end = Instant::now() + Duration::from_secs(10);
    while st.audio().await.inputs.is_empty() {
        assert!(Instant::now() < end, "Mercury's device lists");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(st.h.call(set_audio()).await.unwrap(), CommandOk::Done);
    snap_until(&st.h, 10, "the card chosen", |s| s.builtin_detail.is_none()).await;

    st.deselect().await;
    assert!(closed_within(st.ports.control, Duration::from_secs(6)).await);
    st.select().await;
    let s = snap_until(&st.h, 20, "listening again", listening).await;
    assert_eq!(s.builtin_detail, None);
    let starts = st.starts();
    assert_eq!(starts.len(), 2);
    assert!(!starts[1].contains("-x null"), "{starts:?}");
}

/// M5 §5.4: restarted once after 2 s; a second death stops it, with the log.
#[tokio::test(flavor = "multi_thread")]
async fn dead_mercury_restarts_once_then_stops() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "--die-after 2500").await;
    st.select().await;
    let s = snap_until(&st.h, 30, "Mercury stopped", |s| {
        matches!(s.builtin_detail, Some(ModemDetail::MercuryStopped { .. }))
    })
    .await;
    let Some(ModemDetail::MercuryStopped { reason }) = s.builtin_detail else { unreachable!() };
    assert!(reason.contains("fake-mercury: dying"), "{reason}");
    assert_eq!(st.starts().len(), 2, "one restart only");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(st.starts().len(), 2, "no third start");
}

/// Review Focus 3: Mercury refusing a sound card is its answer, not a crash.
#[tokio::test(flavor = "multi_thread")]
async fn audio_error_is_not_a_crash() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "--audio-fail Device_not_found").await;
    st.select().await;
    snap_until(&st.h, 20, "listening", listening).await;
    let end = Instant::now() + Duration::from_secs(10);
    while st.audio().await.inputs.is_empty() {
        assert!(Instant::now() < end, "Mercury's device lists");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(st.h.call(set_audio()).await.unwrap(), CommandOk::Done);
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let a = st.audio().await;
        if a.audio_ok == Some(false) {
            assert_eq!(a.audio_error.as_deref(), Some("Device_not_found"));
            break;
        }
        assert!(Instant::now() < end, "the audio error: {a:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let s = snap(&st.h).await;
    assert!(listening(&s), "{:?}", s.modem);
    assert_eq!(s.builtin_detail, Some(ModemDetail::ChooseSoundCard));
    assert_eq!(st.starts().len(), 1, "no restart");
}

/// M5 G4: a changed Mercury is refused with the reinstall message.
#[tokio::test(flavor = "multi_thread")]
async fn changed_bundle_refuses_to_start() {
    let root = tempfile::tempdir().unwrap();
    let bundle = test_bundle(root.path());
    let mut f = std::fs::OpenOptions::new().append(true).open(bundle.dir.join(format!("mercury{EXE}"))).unwrap();
    std::io::Write::write_all(&mut f, b"tampered").unwrap();
    drop(f);
    let st = station(root.path(), bundle, "").await;
    st.select().await;
    let s = snap_until(&st.h, 10, "refused", |s| s.modem == ModemState::Faulted).await;
    let msg = "This copy of Mercury has been changed. Reinstall MercuryChat.";
    assert_eq!(s.modem_detail.as_deref(), Some(msg));
    assert_eq!(s.builtin_detail, Some(ModemDetail::MercuryStopped { reason: msg.into() }));
    assert!(st.starts().is_empty());
}

/// M5 W4: the read-only CAT test of a built-in station probes the rigctld
/// MercuryChat runs for it (127.0.0.1 on the profile's rigctld port), not the
/// external rigctld box's address.
#[tokio::test(flavor = "multi_thread")]
async fn rig_test_probes_our_own_rigctld() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    st.select().await;
    snap_until(&st.h, 20, "listening", listening).await;
    assert_eq!(st.h.call(AppCommand::SetRigConfig { config: managed_dummy() }).await.unwrap(), CommandOk::Done);
    snap_until(&st.h, 20, "our rigctld running", |s| s.rig.daemon.as_deref() == Some("running")).await;
    let steps = match st.h.call(AppCommand::TestRig { config: managed_dummy() }).await.unwrap() {
        CommandOk::RigTest { steps } => steps,
        other => panic!("{other:?}"),
    };
    let connect = steps.iter().find(|s| s.name == "connect").expect("a connect step");
    assert_eq!(connect.ok, Some(true), "{steps:?}");
    assert!(connect.detail.contains(&st.ports.rigctld.to_string()), "{steps:?}");
}

/// M5 W4: a built-in station's rig models come from the bundled rigctld's
/// `-l` (fake-rigctld prints Hamlib's table), not from one on the PATH.
#[tokio::test(flavor = "multi_thread")]
async fn hamlib_models_come_from_the_bundled_rigctld() {
    let root = tempfile::tempdir().unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    st.select().await;
    match st.h.call(AppCommand::HamlibModels { hamlib_dir: String::new() }).await.unwrap() {
        CommandOk::HamlibModels { models, found } => {
            assert!(found);
            assert!(models.iter().any(|m| m.id == 1 && m.model == "Dummy"), "{models:?}");
            assert!(models.iter().any(|m| m.id == 3073 && m.model == "IC-7300"), "{models:?}");
        }
        other => panic!("{other:?}"),
    }
}

/// M5 §7: the diagnostics preview lists exactly what the saved zip holds: the
/// versions, the settings without the email, the app's logs and Mercury's
/// log tail; never the database.
#[tokio::test(flavor = "multi_thread")]
async fn diagnostics_preview_matches_the_saved_zip() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("logs")).unwrap();
    std::fs::write(
        root.path().join("logs").join("mercurychat.log.2026-11-23"),
        "INFO attached
",
    )
    .unwrap();
    let st = station(root.path(), test_bundle(root.path()), "").await;
    st.select().await;
    snap_until(&st.h, 20, "listening", listening).await;
    let entries = match st.h.call(AppCommand::DiagnosticsPreview).await.unwrap() {
        CommandOk::Diagnostics { entries } => entries,
        other => panic!("{other:?}"),
    };
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["versions.txt", "profile.json", "logs/mercurychat.log.2026-11-23", "mercury.log", "rigctld.log"]
    );
    let zip = root.path().join("diag.zip").to_string_lossy().into_owned();
    let saved = st.h.call(AppCommand::SaveDiagnostics { path: zip.clone() }).await.unwrap();
    assert_eq!(saved, CommandOk::Exported { count: entries.len() as u32, path: zip.clone() });
    let mut z = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
    let written: Vec<String> =
        (0..z.len()).map(|i| z.by_index(i).unwrap().name().map(|n| n.to_string()).unwrap()).collect();
    assert_eq!(written, names);
    let mut versions = String::new();
    std::io::Read::read_to_string(&mut z.by_name("versions.txt").unwrap(), &mut versions).unwrap();
    assert!(versions.contains("Mercury 1.9.25") && versions.contains("Hamlib 4.7.2"), "{versions}");
}
