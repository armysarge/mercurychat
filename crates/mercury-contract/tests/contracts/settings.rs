//! A2 Settings (M3b.1 section 7): the device lists come on connect; a good
//! set_audio_config keeps the audio up and a bogus device takes it down; a
//! hamlib PTT keys through rigctld; every PTT method is echoed in radio_list.

use mercury_adapter::telemetry::{AudioEvent, Event};
use mercury_adapter::Command;
use mercury_contract::{contract, link, Tag, Target, TargetKind, Ws};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// set_audio_config waits up to 3 s for the audio to come up (WEBSOCKET_API.md 3.1).
fn audio_settle(t: &Target) -> Duration {
    t.reply() + Duration::from_secs(4)
}

/// The capture and playback devices the station uses now.
async fn current_devices(ws: &mut Ws, t: &Target) -> (String, String) {
    let (mut cap, mut play) = (None, None);
    while cap.is_none() || play.is_none() {
        let (_, ev) = ws
            .next(t.reply(), "the device lists", |_, e| {
                matches!(e, Some(Event::Audio(AudioEvent::Capture { .. } | AudioEvent::Playback { .. })))
            })
            .await;
        match ev {
            Some(Event::Audio(AudioEvent::Capture { selected, .. })) => cap = Some(selected),
            Some(Event::Audio(AudioEvent::Playback { selected, .. })) => play = Some(selected),
            _ => unreachable!(),
        }
    }
    (cap.unwrap(), play.unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn device_lists_on_connect() {
    contract("device_lists_on_connect", Tag::Both, |t| async move {
        let mut ws = Ws::open(&t.a).await;
        let mut need = vec!["capture_dev_list", "playback_dev_list", "audio_system", "input_channel"];
        while !need.is_empty() {
            let (raw, ev) = ws
                .next(t.reply(), &format!("{need:?}"), |raw, _| need.iter().any(|k| raw.contains(&format!(r#""{k}""#))))
                .await;
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let kind = v["type"].as_str().unwrap().to_string();
            assert!(v["selected"].is_string(), "{kind} has no selected: {raw}");
            match kind.as_str() {
                "capture_dev_list" | "playback_dev_list" => {
                    let list = v["list"].as_array().unwrap_or_else(|| panic!("{kind} has no list: {raw}"));
                    assert!(list.iter().all(|d| d["id"].is_string() && d["name"].is_string()), "{raw}");
                }
                "audio_system" => {
                    assert!(v["list"].as_array().is_some_and(|l| l.iter().all(|s| s.is_string())), "{raw}")
                }
                _ => {}
            }
            assert!(ev.is_some(), "{kind} does not parse: {raw}");
            need.retain(|k| *k != kind);
        }
    })
    .await;
}

fn audio_is(ok: bool) -> impl Fn(&str, Option<&Event>) -> bool {
    move |_, e| matches!(e, Some(Event::Status(s)) if s.audio_ok == Some(ok))
}

fn audio_config(capture: &str, playback: &str) -> serde_json::Value {
    json!({"command": "set_audio_config", "value": capture, "value2": playback, "value3": "left"})
}

#[tokio::test(flavor = "multi_thread")]
async fn set_audio_config_good_and_bad() {
    contract("set_audio_config_good_and_bad", Tag::Both, |t| async move {
        let mut ws = Ws::open(&t.a).await;
        let (cap, play) = current_devices(&mut ws, &t).await;
        ws.send(audio_config(&cap, &play)).await;
        ws.next(audio_settle(&t), "audio_ok true with the station's own devices", audio_is(true)).await;
        // On real Mercury the ini is checked too: only a config that worked is saved.
        let ini = std::env::var("MC_A_INI").ok().filter(|_| t.kind == TargetKind::Real);
        let read = |path: &str| std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        if let Some(path) = &ini {
            assert!(read(path).contains(&cap), "the good capture device {cap} is not in {path}");
        }
        ws.send(audio_config("mc-contract-bogus", &play)).await;
        ws.next(audio_settle(&t), "audio_ok false with a bogus capture device", audio_is(false)).await;
        if let Some(path) = &ini {
            assert!(!read(path).contains("mc-contract-bogus"), "a failed config was saved to {path}");
        }
        ws.send(audio_config(&cap, &play)).await;
        ws.next(audio_settle(&t), "audio_ok true again", audio_is(true)).await;
    })
    .await;
}

/// One rigctl command on a connection of its own to rigctld.
async fn rigctl(addr: &str, cmd: &str) -> String {
    let sock = tokio::net::TcpStream::connect(addr).await.unwrap_or_else(|e| panic!("rigctld {addr}: {e}"));
    let (r, mut w) = sock.into_split();
    w.write_all(format!("{cmd}\n").as_bytes()).await.unwrap();
    let mut line = String::new();
    BufReader::new(r).read_line(&mut line).await.unwrap();
    line.trim().to_string()
}

fn ptt_config(method: &str, path: &str, model: &str) -> serde_json::Value {
    json!({"command": "set_ptt_config", "value": method, "value2": path, "value3": model,
           "value4": "0", "value5": "rts", "value6": "none", "value7": "3"})
}

#[tokio::test(flavor = "multi_thread")]
async fn set_ptt_config_hamlib_keys_through_rigctld() {
    contract("set_ptt_config_hamlib_keys_through_rigctld", Tag::RealOnly, |t| async move {
        let rig = std::env::var("MC_A_RIGCTLD").expect("MC_A_RIGCTLD (host:port of A's rigctld) is not set");
        let mut ws = Ws::open(&t.a).await;
        // Hamlib model 2 is NET rigctl: Mercury keys through the station's rigctld.
        ws.send(ptt_config("hamlib", &rig, "2")).await;
        ws.next(
            t.reply(),
            "radio_list with hamlib",
            |_, e| matches!(e, Some(Event::RadioList(r)) if r.ptt_method == "hamlib"),
        )
        .await;
        let (mut a, _b) = link(&t).await;
        let end = tokio::time::Instant::now() + t.arq_timeout();
        let mut keyed = false;
        while !keyed && tokio::time::Instant::now() < end {
            a.data.write_all(&[0x55; 200]).await.unwrap();
            for _ in 0..20 {
                if rigctl(&rig, "t").await == "1" {
                    keyed = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let _ = a.client.command(&Command::Abort).await;
        ws.send(ptt_config("none", "", "0")).await;
        assert!(keyed, "rigctld never read PTT 1 during the session");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_ptt_config_methods_echoed() {
    contract("set_ptt_config_methods_echoed", Tag::Both, |t| async move {
        let mut ws = Ws::open(&t.a).await;
        for (method, path) in [("serial", "/dev/null"), ("cm108", ""), ("none", "")] {
            ws.send(ptt_config(method, path, "0")).await;
            ws.next(
                t.reply(),
                &format!("radio_list with {method}"),
                |_, e| matches!(e, Some(Event::RadioList(r)) if r.ptt_method == method),
            )
            .await;
        }
    })
    .await;
}
