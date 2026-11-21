//! A2 Telemetry (WebSocket): radio_list carries its fields; status carries
//! the frequency and SNR; set_tx_gain is confirmed in status.

use mercury_adapter::telemetry::Event;
use mercury_contract::{contract, Tag, Ws};
use serde_json::json;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn radio_list_fields() {
    contract("radio_list_fields", Tag::Both, |t| async move {
        let mut ws = Ws::open(&t.a).await;
        let (raw, ev) = ws.next(t.reply(), "radio_list", |raw, _| raw.contains(r#""radio_list""#)).await;
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for k in
            ["selected", "device_path", "serial_speed", "ptt_method", "ptt_line", "ptt_invert", "cm108_gpio", "list"]
        {
            assert!(v.get(k).is_some(), "radio_list has no {k}: {raw}");
        }
        let Some(Event::RadioList(rl)) = ev else { panic!("radio_list does not parse: {raw}") };
        assert!(!rl.ptt_method.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_frequency_and_snr() {
    contract("status_frequency_and_snr", Tag::Both, |t| async move {
        let mut ws = Ws::open(&t.a).await;
        let (raw, ev) = ws.next(t.reply(), "status", |_, e| matches!(e, Some(Event::Status(_)))).await;
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for k in ["snr", "radio_frequency_hz", "radio_frequency_age_ms", "freq_offset_hz", "freq_offset_age_ms"] {
            assert!(v.get(k).is_some(), "status has no {k}: {raw}");
        }
        let Some(Event::Status(st)) = ev else { unreachable!() };
        assert_eq!(st.radio_frequency_hz, v["radio_frequency_hz"].as_u64());
        if v["radio_frequency_hz"].is_u64() {
            assert!(v["radio_frequency_age_ms"].is_u64(), "a frequency without its age: {raw}");
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_tx_gain_confirmed() {
    contract("set_tx_gain_confirmed", Tag::Both, |t| async move {
        let mut ws = Ws::open(&t.a).await;
        let (raw, _) = ws.next(t.reply(), "status", |_, e| matches!(e, Some(Event::Status(_)))).await;
        let before = serde_json::from_str::<serde_json::Value>(&raw).unwrap()["tx_gain_db"].as_f64().unwrap_or(0.0);
        let want = if (before - 6.5).abs() < 0.01 { 3.0 } else { 6.5 };
        ws.send(json!({"command": "set_tx_gain", "value": want.to_string()})).await;
        ws.next(t.reply() + Duration::from_secs(2), "status with the new TX gain", |_, e| {
            matches!(e, Some(Event::Status(s)) if s.tx_gain_db.is_some_and(|g| (f64::from(g) - want).abs() < 0.01))
        })
        .await;
        ws.send(json!({"command": "set_tx_gain", "value": before.to_string()})).await;
    })
    .await;
}
