//! The built-in modem (M5 §5.7): a Mercury started by MercuryChat's own
//! supervisor from the ini it generates (bind_address = 127.0.0.1,
//! ui_allowed_origins = "", no store), with no sound card or a missing one.
//! Real target only: the fake has no ini, no bind address and no audio.
//!
//! The lab sets `MC_MERCURY_BIN` (the Mercury under test) and `MC_LAB_IP`
//! (the container's own non-loopback address). Ports 8500/8501, 8600 and
//! 10200 are this file's; the pair the other contracts use is left alone.

use mc_core::app::builtin::{BuiltInStart, MercuryPtt, Supervisor};
use mc_core::profile::BuiltInPorts;
use mercury_adapter::telemetry::Event;
use mercury_adapter::{Callsign, Endpoint};
use mercury_contract::{attach, contract, Station, Tag, Target, Ws};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const PORTS: BuiltInPorts = BuiltInPorts { control: 8500, broadcast: 8600, ui: 10200, rigctld: 4700 };

fn mercury() -> PathBuf {
    PathBuf::from(std::env::var("MC_MERCURY_BIN").expect("MC_MERCURY_BIN names the Mercury under test"))
}

fn ws_url() -> String {
    format!("ws://127.0.0.1:{}/websocket", PORTS.ui)
}

fn station() -> Station {
    Station {
        ep: Endpoint::base("127.0.0.1", PORTS.control),
        bcast: SocketAddr::from(([127, 0, 0, 1], PORTS.broadcast)),
        ws: Some(ws_url()),
        call: Callsign::parse("N0CTC").unwrap(),
    }
}

async fn start(dir: &Path, audio_null: bool) -> Supervisor {
    let cfg =
        BuiltInStart { mercury: mercury(), dir: dir.to_path_buf(), ports: PORTS, ptt: MercuryPtt::None, audio_null };
    match Supervisor::start(cfg).await {
        Ok(s) => s,
        Err(e) => panic!("the built-in Mercury did not start: {e:?}"),
    }
}

async fn connects(addr: SocketAddr) -> bool {
    matches!(tokio::time::timeout(Duration::from_secs(2), tokio::net::TcpStream::connect(addr)).await, Ok(Ok(_)))
}

#[tokio::test(flavor = "multi_thread")]
async fn generated_ini_binds_loopback_only() {
    contract("generated_ini_binds_loopback_only", Tag::RealOnly, |_t: Target| async move {
        let dir = tempfile::tempdir().unwrap();
        let sup = start(dir.path(), true).await;
        let ip: std::net::IpAddr =
            std::env::var("MC_LAB_IP").expect("MC_LAB_IP").parse().expect("MC_LAB_IP is an address");
        assert!(!ip.is_loopback(), "MC_LAB_IP must not be loopback");
        for port in [PORTS.control, PORTS.control + 1, PORTS.broadcast, PORTS.ui] {
            assert!(!connects(SocketAddr::new(ip, port)).await, "port {port} answers on {ip}");
        }
        assert!(connects(SocketAddr::from(([127, 0, 0, 1], PORTS.ui))).await, "the UI port on loopback");
        sup.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn generated_ini_refuses_foreign_origin() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    contract("generated_ini_refuses_foreign_origin", Tag::RealOnly, |_t: Target| async move {
        let dir = tempfile::tempdir().unwrap();
        let sup = start(dir.path(), true).await;
        let mut req = ws_url().into_client_request().unwrap();
        req.headers_mut().insert("Origin", "http://evil.example".parse().unwrap());
        match tokio_tungstenite::connect_async(req).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 403),
            Err(e) => panic!("expected 403 for a foreign Origin, got {e}"),
            Ok(_) => panic!("a foreign Origin was upgraded"),
        }
        let plain = tokio_tungstenite::connect_async(ws_url()).await;
        assert!(plain.is_ok(), "no Origin (MercuryChat itself) is upgraded: {:?}", plain.err());
        sup.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn null_audio_reaches_ready() {
    contract("null_audio_reaches_ready", Tag::RealOnly, |t: Target| async move {
        let dir = tempfile::tempdir().unwrap();
        let sup = start(dir.path(), true).await;
        drop(attach(&station(), &t).await);
        sup.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn device_list_with_z() {
    contract("device_list_with_z", Tag::RealOnly, |_t: Target| async move {
        for system in ["null", "pulse"] {
            let out = tokio::time::timeout(
                Duration::from_secs(20),
                tokio::process::Command::new(mercury()).args(["-z", "-x", system]).output(),
            )
            .await
            .unwrap_or_else(|_| panic!("mercury -z -x {system} did not finish"))
            .expect("mercury runs");
            let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(out.status.success(), "mercury -z -x {system}: {}\n{text}", out.status);
            assert!(!text.trim().is_empty(), "mercury -z -x {system} printed nothing");
            println!("NOTE device_list_with_z {system}: {} lines", text.lines().count());
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_device_reports_audio_error() {
    contract("missing_device_reports_audio_error", Tag::RealOnly, |t: Target| async move {
        let dir = tempfile::tempdir().unwrap();
        // What Mercury would have saved for a card that has since gone.
        std::fs::write(
            dir.path().join("mercury.ini"),
            "[main]\nsound_system = pulse\ninput_device = \"no_such_source\"\noutput_device = \"no_such_sink\"\n",
        )
        .unwrap();
        let sup = start(dir.path(), false).await;
        let mut ws = Ws::open(&station()).await;
        ws.next(
            t.reply() + Duration::from_secs(5),
            "audio_ok false",
            |_, e| matches!(e, Some(Event::Status(s)) if s.audio_ok == Some(false)),
        )
        .await;
        drop(attach(&station(), &t).await);
        sup.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn quit_ends_the_process() {
    contract("quit_ends_the_process", Tag::RealOnly, |_t: Target| async move {
        let dir = tempfile::tempdir().unwrap();
        let sup = start(dir.path(), true).await;
        let pid = sup.pid().expect("a running Mercury");
        let t0 = Instant::now();
        sup.stop().await;
        let took = t0.elapsed();
        assert!(!Path::new(&format!("/proc/{pid}")).exists(), "Mercury {pid} is still running");
        assert!(!connects(SocketAddr::from(([127, 0, 0, 1], PORTS.ui))).await, "the UI port still answers");
        println!("NOTE quit_ends_the_process stopped in {} ms", took.as_millis());
    })
    .await;
}
