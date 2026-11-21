//! M3b.1 Task 6: Mercury's sound devices chosen in MercuryChat, through
//! Mercury's UI link (a fake Mercury WebSocket here). Refused while the
//! station is busy; the result is Mercury's own `audio_ok`.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, ActivityEntry, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AudioSettingsView, CommandOk,
    EventEnvelope, ModemState,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::fake_ws::FakeWs;
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::time::Instant;

const STATUS: &str = r#"{"type":"status","audio_ok":true,"audio_error":""}"#;
const CAPTURE: &str = r#"{"type":"capture_dev_list","selected":"in1","list":[{"name":"Line In","id":"in1"},{"name":"USB CODEC","id":"in2"}]}"#;
const PLAYBACK: &str = r#"{"type":"playback_dev_list","selected":"out1","list":[{"name":"Speakers","id":"out1"},{"name":"USB CODEC","id":"out2"}]}"#;
const SYSTEM: &str = r#"{"type":"audio_system","selected":"wasapi","list":["wasapi"]}"#;
const CHANNEL: &str = r#"{"type":"input_channel","selected":"left","list":["left","right","stereo"]}"#;

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

fn existing(net: &FakeNet, telemetry_url: Option<String>) -> StationProfile {
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: net.a_base,
        data_port: net.a_base + 1,
        broadcast_port: Some(net.a_bcast),
        telemetry_url,
    };
    p
}

async fn fake_mercury_ws() -> FakeWs {
    let ws = FakeWs::start().await;
    ws.set_status(Some(STATUS));
    ws.set_hello(vec![CAPTURE.into(), PLAYBACK.into(), SYSTEM.into(), CHANNEL.into()]);
    ws
}

async fn start(root: &Path, profile: StationProfile) -> (AppHandle, Receiver<EventEnvelope>) {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(root));
    let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        let ev = tokio::time::timeout_at(end, rx.recv()).await.expect("listening in time").unwrap();
        if let AppEvent::ModemState { state: ModemState::Listening, .. } = ev.event {
            break;
        }
    }
    (h, rx)
}

async fn audio(h: &AppHandle) -> AudioSettingsView {
    match h.call(AppCommand::AudioSettings).await.unwrap() {
        CommandOk::Audio(v) => v,
        other => panic!("{other:?}"),
    }
}

async fn audio_until(h: &AppHandle, what: &str, pred: impl Fn(&AudioSettingsView) -> bool) -> AudioSettingsView {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let v = audio(h).await;
        if pred(&v) {
            return v;
        }
        assert!(Instant::now() < end, "timed out waiting for {what}: {v:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn note(rx: &mut Receiver<EventEnvelope>, contains: &str) -> String {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let ev = tokio::time::timeout_at(end, rx.recv()).await.expect("the note in time").unwrap();
        if let AppEvent::Activity { entry: ActivityEntry { text, .. } } = ev.event {
            if text.contains(contains) {
                return text;
            }
        }
    }
}

fn set_audio(input: &str, output: &str) -> AppCommand {
    AppCommand::SetAudio { system: "wasapi".into(), input: input.into(), output: output.into(), channel: "left".into() }
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_view_from_events() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = fake_mercury_ws().await;
    let (h, _rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    let v = audio_until(&h, "the device lists", |v| v.available && v.inputs.len() == 2 && v.outputs.len() == 2).await;
    assert_eq!((v.input.as_str(), v.output.as_str()), ("in1", "out1"));
    assert_eq!((v.system.as_str(), v.channel.as_str()), ("wasapi", "left"));
    assert_eq!(v.systems, vec!["wasapi".to_string()]);
    assert_eq!(v.inputs[1].name, "USB CODEC");
    assert_eq!(v.audio_ok, Some(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_unavailable_without_link() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let (h, _rx) = start(dir.path(), existing(&net, None)).await;
    let v = audio(&h).await;
    assert!(!v.available && v.inputs.is_empty());
    match h.call(set_audio("in2", "out2")).await {
        Err(AppError::Conflict { message }) => assert!(message.contains("-G"), "{message}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_refused_while_busy() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = fake_mercury_ws().await;
    let (h, _rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    audio_until(&h, "the link", |v| v.available).await;
    h.call(AppCommand::Call { peer: "ZS1XYZ".into() }).await.unwrap();
    match h.call(set_audio("in2", "out2")).await {
        Err(AppError::Refused { reason, .. }) => assert_eq!(reason, "rig.refused.calling"),
        other => panic!("{other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!ws.received().iter().any(|t| t.contains("set_audio_config")), "{:?}", ws.received());
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_bad_channel_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = fake_mercury_ws().await;
    let (h, _rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    audio_until(&h, "the link", |v| v.available).await;
    let cmd = AppCommand::SetAudio {
        system: "wasapi".into(),
        input: "in2".into(),
        output: "out2".into(),
        channel: "mono".into(),
    };
    assert!(matches!(h.call(cmd).await, Err(AppError::Validation { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_applied_note() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = fake_mercury_ws().await;
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    audio_until(&h, "the link", |v| v.available).await;
    assert_eq!(h.call(set_audio("in2", "out2")).await.unwrap(), CommandOk::Done);
    let text = note(&mut rx, "Audio").await;
    assert!(text.contains("applied") && !text.contains("not applied"), "{text}");
    let sent: Vec<serde_json::Value> = ws
        .received()
        .iter()
        .filter_map(|t| serde_json::from_str::<serde_json::Value>(t).ok())
        .filter(|v| v["command"] == "set_audio_config")
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0]["value"].as_str(), sent[0]["value2"].as_str()), (Some("in2"), Some("out2")));
    audio_until(&h, "the new input", |v| v.input == "in2").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn audio_failed_note() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = fake_mercury_ws().await;
    ws.set_audio_result(false, "device busy");
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    audio_until(&h, "the link", |v| v.available).await;
    h.call(set_audio("in2", "out2")).await.unwrap();
    let text = note(&mut rx, "Audio not applied").await;
    assert!(text.contains("device busy"), "{text}");
}
