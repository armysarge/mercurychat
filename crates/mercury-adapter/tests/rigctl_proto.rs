//! rigctld's extended protocol against golden transcripts from rigctld 4.6.2
//! (tests/fixtures/rigctld/README.md).

use mercury_adapter::rigctl::{block_complete, parse_reply, Level, RigProtoError, RigReply, RigRequest};

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/rigctld/{name}.txt", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn all_requests() -> Vec<RigRequest> {
    vec![
        RigRequest::GetFreq,
        RigRequest::SetFreq(7_105_000),
        RigRequest::GetMode,
        RigRequest::SetMode { mode: "PKTUSB".into(), passband_hz: 3000 },
        RigRequest::GetVfo,
        RigRequest::SetVfo("VFOA".into()),
        RigRequest::GetSplit,
        RigRequest::SetSplit { on: true, tx_vfo: "VFOB".into() },
        RigRequest::GetPtt,
        RigRequest::PttOff,
        RigRequest::GetLevel(Level::RfPower),
        RigRequest::SetLevel(Level::RfPower, 0.5),
        RigRequest::DumpCaps,
    ]
}

#[test]
fn wire_forms_are_extended_and_never_key() {
    let want = [
        "+f\n",
        "+F 7105000\n",
        "+m\n",
        "+M PKTUSB 3000\n",
        "+v\n",
        "+V VFOA\n",
        "+s\n",
        "+S 1 VFOB\n",
        "+t\n",
        "+T 0\n",
        "+l RFPOWER\n",
        "+L RFPOWER 0.5\n",
        "+\\dump_caps\n",
    ];
    let reqs = all_requests();
    assert_eq!(reqs.len(), want.len());
    for (r, w) in reqs.iter().zip(want) {
        assert_eq!(r.wire(), w, "{r:?}");
        assert!(!r.wire().contains("T 1"), "{r:?} would key the rig");
    }
    assert_eq!(Level::Swr.token(), "SWR");
    assert_eq!(Level::Alc.token(), "ALC");
    assert_eq!(Level::RfPowerMeter.token(), "RFPOWER_METER");
}

#[test]
fn golden_replies_parse() {
    let ok =
        |req: RigRequest, name: &str| parse_reply(&req, &fixture(name)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!(ok(RigRequest::GetFreq, "get_freq"), RigReply::Freq(145_000_000));
    assert_eq!(ok(RigRequest::SetFreq(7_105_000), "set_freq"), RigReply::Ok);
    assert_eq!(ok(RigRequest::GetMode, "get_mode"), RigReply::Mode { mode: "FM".into(), passband_hz: 15000 });
    assert_eq!(ok(RigRequest::SetMode { mode: "PKTUSB".into(), passband_hz: 3000 }, "set_mode"), RigReply::Ok);
    assert_eq!(ok(RigRequest::GetVfo, "get_vfo"), RigReply::Vfo("VFOA".into()));
    assert_eq!(ok(RigRequest::GetSplit, "get_split"), RigReply::Split { on: false, tx_vfo: "None".into() });
    assert_eq!(ok(RigRequest::GetPtt, "get_ptt_off"), RigReply::Ptt(false));
    assert_eq!(ok(RigRequest::GetPtt, "get_ptt_on"), RigReply::Ptt(true));
    assert_eq!(ok(RigRequest::GetLevel(Level::RfPower), "get_level_rfpower"), RigReply::Level(0.0));
    assert_eq!(ok(RigRequest::SetLevel(Level::RfPower, 0.5), "set_level_rfpower"), RigReply::Ok);
    let RigReply::Caps(caps) = ok(RigRequest::DumpCaps, "dump_caps") else { panic!("not caps") };
    assert_eq!(caps.model, "Dummy");
    assert!(caps.can_set_freq && caps.can_set_mode && caps.can_set_vfo && caps.can_split && caps.can_get_ptt);
    assert!(caps.set_levels.contains(&Level::RfPower));
    for l in [Level::RfPower, Level::Swr, Level::Alc, Level::RfPowerMeter] {
        assert!(caps.get_levels.contains(&l), "{l:?}");
    }
    assert!(!caps.set_levels.contains(&Level::Swr), "meters are read-only");
    assert!(caps.modes.iter().any(|m| m == "USB"), "{:?}", caps.modes);
}

#[test]
fn rprt_errors_are_errors() {
    assert_eq!(parse_reply(&RigRequest::GetPtt, &fixture("get_ptt_rprt_11")), Err(RigProtoError::Rprt(-11)));
    assert_eq!(parse_reply(&RigRequest::SetVfo("VFOZ".into()), &fixture("rprt_err")), Err(RigProtoError::Rprt(-1)));
}

#[test]
fn partial_blocks_are_incomplete() {
    for name in [
        "get_freq",
        "set_freq",
        "get_mode",
        "set_mode",
        "get_vfo",
        "get_split",
        "get_ptt_off",
        "get_ptt_on",
        "get_ptt_rprt_11",
        "get_level_rfpower",
        "set_level_rfpower",
        "dump_caps",
        "rprt_err",
    ] {
        let text = fixture(name);
        assert!(block_complete(&text), "{name}");
        let at = text.rfind("RPRT").unwrap();
        for cut in 0..=at {
            if text.is_char_boundary(cut) {
                assert!(!block_complete(&text[..cut]), "{name} cut at {cut}");
            }
        }
        assert_eq!(parse_reply(&RigRequest::GetFreq, &text[..at]), Err(RigProtoError::Incomplete), "{name}");
    }
}

#[test]
fn garbage_is_malformed() {
    assert!(matches!(
        parse_reply(&RigRequest::GetFreq, "get_freq:\nFrequency: abc\nRPRT 0\n"),
        Err(RigProtoError::Malformed(_))
    ));
    assert!(matches!(parse_reply(&RigRequest::GetPtt, "get_ptt:\nPTT: 7\nRPRT 0\n"), Err(RigProtoError::Malformed(_))));
    assert!(matches!(
        parse_reply(&RigRequest::GetFreq, "get_ptt:\nPTT: 0\nRPRT 0\n"),
        Err(RigProtoError::Malformed(_))
    ));
}
