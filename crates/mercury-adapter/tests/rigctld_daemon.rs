//! MercuryChat's own rigctld: the command line, Hamlib's model list (M3b.1
//! Task 2) and the process supervisor (Task 3).

use mercury_adapter::rigctl::{parse_model_list, rigctld_args, RigctldSpec};

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn conf(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter().map(|(k, x)| (k.to_string(), x.to_string())).collect()
}

#[test]
fn args_serial_full() {
    let spec = RigctldSpec {
        model: 3073,
        rig_path: "COM10".into(),
        baud: Some(9600),
        conf: conf(&[
            ("serial_parity", "None"),
            ("data_bits", "8"),
            ("stop_bits", "1"),
            ("post_write_delay", "100"),
            ("dtr_state", "OFF"),
            ("rts_state", "OFF"),
        ]),
        ptt_type: "RTS",
        ptt_path: None,
        listen: "127.0.0.1:4532".parse().unwrap(),
    };
    assert_eq!(
        rigctld_args(&spec),
        s(&[
            "-m",
            "3073",
            "-r",
            "COM10",
            "-s",
            "9600",
            "-C",
            "serial_parity=None,data_bits=8,stop_bits=1,post_write_delay=100,dtr_state=OFF,rts_state=OFF",
            "-P",
            "RTS",
            "-T",
            "127.0.0.1",
            "-t",
            "4532",
        ])
    );
}

#[test]
fn args_tcp_rig_has_no_baud_or_conf() {
    let spec = RigctldSpec {
        model: 3073,
        rig_path: "192.168.1.20:50001".into(),
        baud: None,
        conf: vec![],
        ptt_type: "RIG",
        ptt_path: None,
        listen: "127.0.0.1:4532".parse().unwrap(),
    };
    let a = rigctld_args(&spec);
    assert!(!a.contains(&"-s".to_string()), "{a:?}");
    assert!(!a.contains(&"-C".to_string()), "{a:?}");
    assert_eq!(a, s(&["-m", "3073", "-r", "192.168.1.20:50001", "-P", "RIG", "-T", "127.0.0.1", "-t", "4532"]));
}

#[test]
fn args_separate_ptt_port() {
    let spec = RigctldSpec {
        model: 1,
        rig_path: "COM10".into(),
        baud: Some(4800),
        conf: vec![],
        ptt_type: "DTR",
        ptt_path: Some("COM11".into()),
        listen: "127.0.0.1:4532".parse().unwrap(),
    };
    let a = rigctld_args(&spec);
    let p = a.iter().position(|x| x == "-P").unwrap();
    assert_eq!(&a[p..p + 4], &s(&["-P", "DTR", "-p", "COM11"])[..]);
}

#[test]
fn models_parse_fixture() {
    let text = include_str!("fixtures/rigctld/list-4.6.txt");
    let models = parse_model_list(text);
    assert!(models.len() > 200, "{}", models.len());
    let dummy = models.iter().find(|m| m.id == 1).expect("dummy");
    assert_eq!((dummy.mfg.as_str(), dummy.model.as_str()), ("Hamlib", "Dummy"));
    let ic = models.iter().find(|m| m.id == 3073).expect("IC-7300");
    assert_eq!((ic.mfg.as_str(), ic.model.as_str(), ic.status.as_str()), ("Icom", "IC-7300", "Stable"));
    let net = models.iter().find(|m| m.id == 2).expect("NET rigctl");
    assert_eq!(net.model, "NET rigctl", "names with spaces stay whole");
    assert!(!models.iter().any(|m| m.mfg == "Mfg"), "header skipped");
}

#[test]
fn models_ignore_garbage() {
    assert!(parse_model_list("\n  xx\nRig #\n").is_empty());
    assert!(parse_model_list("").is_empty());
}
