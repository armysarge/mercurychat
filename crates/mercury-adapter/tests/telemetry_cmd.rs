//! M3b Task 6: the one command MercuryChat sends on Mercury's telemetry
//! WebSocket (`set_tx_gain`), and the `radio_list` event it reads.

use mercury_adapter::fake_ws::FakeWs;
use mercury_adapter::telemetry::{
    parse_event, Event, RadioList, TelemetryClient, TelemetryCmd, TelemetryLink, TelemetryMsg,
};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

async fn until(
    rx: &mut mpsc::Receiver<TelemetryMsg>,
    secs: u64,
    mut pred: impl FnMut(&TelemetryMsg) -> bool,
) -> TelemetryMsg {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        let m = tokio::time::timeout_at(end, rx.recv()).await.expect("telemetry in time").expect("client running");
        if pred(&m) {
            return m;
        }
    }
}

#[test]
fn radio_list_parses() {
    // WEBSOCKET_API.md §5.5's own example.
    let doc = r#"{"type":"radio_list","selected":"","device_path":"/dev/ttyUSB0","serial_speed":9600,
      "ptt_method":"serial","ptt_line":"rts","ptt_invert":"none","cm108_gpio":1,"list":[{"name":"None","id":"-1"}]}"#;
    assert_eq!(
        parse_event(doc).unwrap(),
        Some(Event::RadioList(RadioList {
            ptt_method: "serial".into(),
            selected_model: None,
            device_path: "/dev/ttyUSB0".into(),
            ptt_line: "rts".into(),
            ptt_invert: "none".into(),
            cm108_gpio: Some(1),
        }))
    );
    let net = r#"{"type":"radio_list","selected":"2","device_path":"127.0.0.1:4532","ptt_method":"hamlib","list":[]}"#;
    assert_eq!(
        parse_event(net).unwrap(),
        Some(Event::RadioList(RadioList {
            ptt_method: "hamlib".into(),
            selected_model: Some(2),
            device_path: "127.0.0.1:4532".into(),
            ..RadioList::default()
        }))
    );
    assert!(matches!(parse_event(r#"{"type":"status","snr":1.5}"#).unwrap(), Some(Event::Status(_))));
    assert_eq!(parse_event(r#"{"type":"history"}"#).unwrap(), None);
    assert_eq!(parse_event(r#"{"type":"radio_list"}"#).unwrap(), None, "no ptt_method: not a usable radio list");
}

#[tokio::test(flavor = "multi_thread")]
async fn set_tx_gain_is_sent_as_json_and_radio_list_arrives() {
    let ws = FakeWs::start().await;
    ws.set_status(Some(r#"{"type":"status","snr":3.5,"tx_gain_db":0.0}"#));
    ws.set_radio_list(Some(
        r#"{"type":"radio_list","selected":"2","device_path":"127.0.0.1:4532","ptt_method":"hamlib"}"#,
    ));
    let (tx, mut rx) = mpsc::channel(64);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let (spectrum, _keep) = watch::channel(None);
    let client = TelemetryClient::spawn_commanded(ws.url.clone(), tx, spectrum, cmd_rx);
    until(&mut rx, 5, |m| *m == TelemetryMsg::Link(TelemetryLink::Up)).await;
    let m = until(&mut rx, 5, |m| matches!(m, TelemetryMsg::RadioList(_))).await;
    assert!(matches!(m, TelemetryMsg::RadioList(RadioList { selected_model: Some(2), .. })), "{m:?}");
    cmd_tx.send(TelemetryCmd::SetTxGain(-3.5)).await.unwrap();
    until(&mut rx, 5, |m| matches!(m, TelemetryMsg::Status(s) if s.tx_gain_db == Some(-3.5))).await;
    assert!(ws.received().iter().any(|t| t == r#"{"command":"set_tx_gain","value":"-3.50"}"#), "{:?}", ws.received());
    client.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_while_down_fails() {
    let ws = FakeWs::start().await;
    let url = ws.url.clone();
    drop(ws); // nothing listens
    let (tx, mut rx) = mpsc::channel(64);
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let (spectrum, _keep) = watch::channel(None);
    let client = TelemetryClient::spawn_commanded(url, tx, spectrum, cmd_rx);
    until(&mut rx, 10, |m| matches!(m, TelemetryMsg::Link(TelemetryLink::Lost { .. }))).await;
    cmd_tx.send(TelemetryCmd::SetTxGain(1.0)).await.unwrap();
    let m = until(&mut rx, 5, |m| matches!(m, TelemetryMsg::CommandFailed { .. })).await;
    assert!(matches!(&m, TelemetryMsg::CommandFailed { reason } if reason.contains("not connected")), "{m:?}");
    client.abort();
}
