//! Mercury's sound devices over its UI link (M3b.1 Task 6): the device lists
//! it publishes, and `set_audio_config` as MercuryChat sends it.

use mercury_adapter::telemetry::{parse_event, AudioDevice, AudioEvent, Event, TelemetryCmd};

#[test]
fn parses_device_lists() {
    let json = r#"{"type":"capture_dev_list","selected":"hw:1","list":[
        {"name":"PCM2901 \"A\" [hw:1]","id":"hw:1"},{"name":"Built-in","id":"default"}]}"#;
    let Some(Event::Audio(AudioEvent::Capture { selected, list })) = parse_event(json).unwrap() else {
        panic!("not a capture list")
    };
    assert_eq!(selected, "hw:1");
    assert_eq!(
        list,
        vec![
            AudioDevice { id: "hw:1".into(), name: "PCM2901 \"A\" [hw:1]".into() },
            AudioDevice { id: "default".into(), name: "Built-in".into() },
        ]
    );
    let pb =
        r#"{"type":"playback_dev_list","selected":"","list":[{"name":"Zoë's card","id":"{0.0.0.00000000}.{abc}"}]}"#;
    let Some(Event::Audio(AudioEvent::Playback { selected, list })) = parse_event(pb).unwrap() else { panic!() };
    assert_eq!(selected, "");
    assert_eq!(list[0].name, "Zoë's card", "non-ASCII names stay whole");
    assert_eq!(list[0].id, "{0.0.0.00000000}.{abc}");
}

#[test]
fn parses_system_and_channel() {
    let sys = r#"{"type":"audio_system","selected":"wasapi","list":["wasapi","dsound"]}"#;
    assert_eq!(
        parse_event(sys).unwrap(),
        Some(Event::Audio(AudioEvent::System {
            selected: "wasapi".into(),
            list: vec!["wasapi".into(), "dsound".into()]
        }))
    );
    let ch = r#"{"type":"input_channel","selected":"right","list":["left","right","stereo"]}"#;
    assert_eq!(parse_event(ch).unwrap(), Some(Event::Audio(AudioEvent::Channel { selected: "right".into() })));
}

#[test]
fn device_list_is_bounded() {
    let many: Vec<String> = (0..200).map(|i| format!(r#"{{"name":"d{i}","id":"{i}"}}"#)).collect();
    let json = format!(r#"{{"type":"capture_dev_list","selected":"","list":[{}]}}"#, many.join(","));
    let Some(Event::Audio(AudioEvent::Capture { list, .. })) = parse_event(&json).unwrap() else { panic!() };
    assert_eq!(list.len(), 64, "at most 64 devices are kept");
    let bad = r#"{"type":"capture_dev_list","selected":"","list":[{"name":"x"},{"id":"y","name":"\u0007bell"}]}"#;
    let Some(Event::Audio(AudioEvent::Capture { list, .. })) = parse_event(bad).unwrap() else { panic!() };
    assert_eq!(
        list,
        vec![AudioDevice { id: "y".into(), name: "bell".into() }],
        "no id: dropped; control chars: removed"
    );
}

#[test]
fn audio_json_escapes_quotes_and_backslashes() {
    let cmd = TelemetryCmd::SetAudioConfig {
        capture: r#"a"b\c"#.into(),
        playback: "out".into(),
        channel: "left".into(),
        system: "wasapi".into(),
    };
    let v: serde_json::Value = serde_json::from_str(&cmd.json()).expect("valid JSON");
    assert_eq!(v["command"], "set_audio_config");
    assert_eq!(v["value"], r#"a"b\c"#);
    assert_eq!(v["value2"], "out");
    assert_eq!(v["value3"], "left");
    assert_eq!(v["value4"], "wasapi");
}

// ---- Task 7: Mercury's PTT setting -------------------------------------------

#[test]
fn ptt_json_hamlib() {
    let cmd = TelemetryCmd::SetPttConfig {
        method: "hamlib".into(),
        device: "127.0.0.1:4532".into(),
        model: 2,
        speed: 0,
        line: "rts".into(),
        invert: "none".into(),
        gpio: 3,
    };
    let got: serde_json::Value = serde_json::from_str(&cmd.json()).unwrap();
    let want: serde_json::Value = serde_json::from_str(
        r#"{"command":"set_ptt_config","value":"hamlib","value2":"127.0.0.1:4532","value3":"2","value4":"0","value5":"rts","value6":"none","value7":"3"}"#,
    )
    .unwrap();
    assert_eq!(got, want);
}

#[test]
fn radio_list_reads_serial_and_cm108_fields() {
    let json = r#"{"type":"radio_list","selected":"0","device_path":"COM11","serial_speed":0,"ptt_method":"serial",
        "ptt_line":"dtr","ptt_invert":"rts","cm108_gpio":4,"list":[]}"#;
    let Some(Event::RadioList(rl)) = parse_event(json).unwrap() else { panic!() };
    assert_eq!((rl.ptt_method.as_str(), rl.device_path.as_str()), ("serial", "COM11"));
    assert_eq!((rl.ptt_line.as_str(), rl.ptt_invert.as_str(), rl.cm108_gpio), ("dtr", "rts", Some(4)));
}
