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
                .args(["build", "-p", "mercury-adapter", "--bin", "fake-mercury"])
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
