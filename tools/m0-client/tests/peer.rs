//! M2 Task 9: `m0-client peer` is a headless MercuryChat station for the
//! real-Mercury labs. Here it runs against the in-process fake pair: it
//! answers, echoes, and logs one JSON line per received message, so the
//! bytes can be compared without trusting any display.
use mc_core::{SessionCommand, SessionEvent, Station, StationConfig};
use mch_protocol::ObjectId;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread")]
async fn peer_echo_over_fake_pair() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut child = Command::new(env!("CARGO_BIN_EXE_m0-client"))
        .args(["peer", "--host", "127.0.0.1", "--port", &net.b_base.to_string(), "--call", "SIM2", "--echo", "--work"])
        .arg(dir.path().join("peer"))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("run m0-client peer");
    let stdout = child.stdout.take().unwrap();
    let (line_tx, mut line_rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = line_tx.send(line);
        }
    });
    // The peer says when it is listening.
    let ready = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(l) = line_rx.recv().await {
            if l.contains("\"listening\"") {
                return;
            }
        }
        panic!("the peer exited before listening");
    });
    ready.await.expect("the peer listens within 20 s");

    let call = Callsign::parse("ZS6ABC").unwrap();
    let mut st =
        Station::open(StationConfig::defaults(call, Endpoint::base("127.0.0.1", net.a_base), &dir.path().join("a")))
            .unwrap();
    st.attach().await.unwrap();
    let info = st.call(&Callsign::parse("SIM2").unwrap(), Duration::from_secs(10)).await.unwrap();
    let text = "héllo 👋\r\nline two\n\tשלום";
    let id = ObjectId::random();
    let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
    let (_req_tx, mut req_rx) = mpsc::channel(1);
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    cmd_tx.send(SessionCommand::SendChat { id, text: text.into() }).await.unwrap();
    let echo = async {
        while let Some(ev) = ev_rx.recv().await {
            if let SessionEvent::ChatReceived { text, .. } = ev {
                return text;
            }
        }
        String::new()
    };
    let got = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::select! {
            t = echo => t,
            end = st.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx) => panic!("session ended first: {end:?}"),
        }
    })
    .await
    .expect("an echo within 15 s");
    assert_eq!(got, format!("echo: {text}"));

    // The peer's own record of what it stored.
    let logged = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(l) = line_rx.recv().await {
            if l.contains("\"received\"") {
                return l;
            }
        }
        String::new()
    })
    .await
    .expect("a received line");
    let _ = child.kill();
    let _ = child.wait();
    let v: serde_json::Value = serde_json::from_str(&logged).unwrap();
    assert_eq!(v["event"], "received");
    assert_eq!(v["id"], id.to_hex());
    assert_eq!(v["peer"], "ZS6ABC");
    assert_eq!(v["bytes"], text.len());
    assert_eq!(v["sha256"], hex::encode(Sha256::digest(text.as_bytes())));
}
