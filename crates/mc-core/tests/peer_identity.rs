//! M2 Task 7 (Review Focus 4): the peer callsign Mercury reports is untrusted
//! input. Anything that is not a valid callsign is refused before a session,
//! message or dedup key exists; valid ones are normalised (upper case, suffix
//! kept).

use mc_core::app::{spawn_app, AppCommand, AppEvent, AppOptions, CommandOk, EventEnvelope, ModemState};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig, StationError};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::time::Duration;

async fn station(net: &FakeNet, dir: &Path) -> Station {
    let call = Callsign::parse("ZS6ABC").unwrap();
    let mut st = Station::open(StationConfig::defaults(call, Endpoint::base("127.0.0.1", net.a_base), dir)).unwrap();
    st.attach().await.unwrap();
    st.set_listen(true).await.unwrap();
    st
}

#[tokio::test]
async fn invalid_peer_callsigns_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut st = station(&net, dir.path()).await;
    for bad in ["ZS6ABC;", "ABCDEFGHIJKLMNOP", "ZS6-;A", "--"] {
        net.raw_connected(0, &format!("CONNECTED {bad} ZS6ABC 2300"));
        match st.wait_incoming(Duration::from_secs(2)).await {
            Err(StationError::BadPeer(p)) => assert_eq!(p, bad),
            other => panic!("{bad:?}: expected BadPeer, got {other:?}"),
        }
        st.abort().await.unwrap();
    }
    // Spaces, line breaks and a missing callsign never parse as CONNECTED at all.
    for line in ["CONNECTED ZS6ABC LISTEN ON ZS6ABC 2300", "CONNECTED ZS6\r\nABC ZS6ABC 2300", "CONNECTED  ZS6ABC 2300"]
    {
        net.raw_connected(0, line);
        match st.wait_incoming(Duration::from_millis(500)).await {
            Err(StationError::Timeout(_)) => {}
            other => panic!("{line:?}: expected no connection, got {other:?}"),
        }
        st.abort().await.unwrap();
    }
}

#[tokio::test]
async fn peer_callsign_is_normalised() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut st = station(&net, dir.path()).await;
    for (reported, want) in [("zs1xyz", "ZS1XYZ"), ("zs1xyz/p", "ZS1XYZ/P"), ("n0call-1", "N0CALL-1")] {
        net.raw_connected(0, &format!("CONNECTED {reported} ZS6ABC 2300"));
        let info = st.wait_incoming(Duration::from_secs(2)).await.unwrap();
        assert_eq!(info.peer, want);
        st.abort().await.unwrap();
    }
}

#[tokio::test]
async fn over_long_control_line_is_a_fault() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut st = station(&net, dir.path()).await;
    net.send_control(0, &"A".repeat(mercury_adapter::control::MAX_LINE + 1));
    match st.wait_incoming(Duration::from_secs(2)).await {
        Err(StationError::ControlLost) => {}
        other => panic!("expected the control connection to be treated as lost, got {other:?}"),
    }
}

async fn next(rx: &mut tokio::sync::broadcast::Receiver<EventEnvelope>) -> EventEnvelope {
    tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.expect("an event").unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_peer_link_is_aborted_and_explained() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let opts = AppOptions { stations_root: Some(dir.path().join("stations")), ..AppOptions::default() };
    let h = spawn_app(Store::open_in_memory().unwrap(), opts);
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: net.a_base,
        data_port: net.a_base + 1,
        broadcast_port: None,
        telemetry_url: None,
    };
    let id = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();
    loop {
        let env = next(&mut rx).await;
        if let AppEvent::ModemState { state: ModemState::Listening, .. } = env.event {
            break;
        }
    }

    net.raw_connected(0, "CONNECTED ZS6ABC; ZS6ABC 2300");
    let mut back_to_listening = false;
    let mut failed = None;
    while !(back_to_listening && failed.is_some()) {
        match next(&mut rx).await.event {
            AppEvent::CallFailed { peer, reason } => failed = Some((peer, reason)),
            AppEvent::Session { session: Some(s) } => panic!("a session began with {s:?}"),
            AppEvent::MessageChanged { message } => panic!("a message appeared: {message:?}"),
            AppEvent::ModemState { state: ModemState::Negotiating | ModemState::Connected, .. } => {
                panic!("the link was taken up")
            }
            AppEvent::ModemState { state: ModemState::Listening, .. } if failed.is_some() => back_to_listening = true,
            _ => {}
        }
    }
    assert_eq!(failed.unwrap(), ("ZS6ABC;".to_string(), "peer callsign rejected".to_string()));
    assert!(net.log().iter().any(|l| l == "A> ABORT"), "the link was not aborted");
    let store = Store::open(&dir.path().join("stations").join(&id).join("station.db")).unwrap();
    assert!(store.conversations().unwrap().is_empty());
    assert!(store.messages("ZS6ABC;").unwrap().is_empty());
}
