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

// ---- Task 3: the supervisor -------------------------------------------------

mod supervisor {
    use mercury_adapter::fake_rig::FakeRigctld;
    use mercury_adapter::rigctl::{find_rigctld_in, DaemonState, RigClient, RigMsg, RigctldDaemon, RigctldSpec};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::time::Duration;
    use tokio::sync::mpsc;

    const FAKE: &str = env!("CARGO_BIN_EXE_fake-rigctld");

    fn free_addr() -> SocketAddr {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    }

    fn spec(listen: SocketAddr) -> RigctldSpec {
        RigctldSpec {
            model: 1,
            rig_path: "COM10".into(),
            baud: Some(9600),
            conf: vec![],
            ptt_type: "RIG",
            ptt_path: None,
            listen,
        }
    }

    fn start(listen: SocketAddr, env: &[(&str, &str)]) -> (RigctldDaemon, mpsc::Receiver<DaemonState>) {
        let (tx, rx) = mpsc::channel(16);
        let env = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        (RigctldDaemon::start_with_env(PathBuf::from(FAKE), spec(listen), tx, env), rx)
    }

    async fn next(rx: &mut mpsc::Receiver<DaemonState>) -> DaemonState {
        tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("a state in time").expect("open")
    }

    async fn answers(addr: SocketAddr) -> bool {
        tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect(addr))
            .await
            .is_ok_and(|r| r.is_ok())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn starts_and_runs() {
        let addr = free_addr();
        let (d, mut rx) = start(addr, &[]);
        assert_eq!(next(&mut rx).await, DaemonState::Starting);
        assert_eq!(next(&mut rx).await, DaemonState::Running);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(d.log().iter().any(|l| l == "fake-rigctld: ready"), "{:?}", d.log());
        let (_ctx, crx) = mpsc::channel(4);
        let (otx, mut orx) = mpsc::channel(64);
        let _c = RigClient::spawn(addr.to_string(), crx, otx);
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(RigMsg::Freq(hz)) = orx.recv().await {
                    break hz;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(got, 14_074_000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn foreign_daemon_is_reused_and_not_killed() {
        let fake = FakeRigctld::start().await;
        let (d, mut rx) = start(fake.addr, &[]);
        assert_eq!(next(&mut rx).await, DaemonState::Reused);
        drop(d);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(answers(fake.addr).await, "someone else's rigctld is left running");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn early_exit_reports_stderr() {
        let (_d, mut rx) = start(free_addr(), &[("FAKE_RIGCTLD_FAIL", "rig_open: error = IO error")]);
        assert_eq!(next(&mut rx).await, DaemonState::Starting);
        assert_eq!(next(&mut rx).await, DaemonState::Restarting);
        match next(&mut rx).await {
            DaemonState::Failed { reason } => assert!(reason.contains("IO error"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn drop_kills_owned() {
        let addr = free_addr();
        let (d, mut rx) = start(addr, &[]);
        assert_eq!(next(&mut rx).await, DaemonState::Starting);
        assert_eq!(next(&mut rx).await, DaemonState::Running);
        drop(d);
        let gone = tokio::time::timeout(Duration::from_secs(2), async {
            while answers(addr).await {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(gone.is_ok(), "MercuryChat's own rigctld is stopped with it");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn log_keeps_200() {
        let (d, mut rx) = start(free_addr(), &[("FAKE_RIGCTLD_SPAM", "250")]);
        assert_eq!(next(&mut rx).await, DaemonState::Starting);
        assert_eq!(next(&mut rx).await, DaemonState::Running);
        let ok = tokio::time::timeout(Duration::from_secs(5), async {
            while d.log().last().map(String::as_str) != Some("line 250") {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        assert!(ok.is_ok(), "{:?}", d.log().last());
        assert_eq!(d.log().len(), 200);
    }

    #[test]
    fn find_prefers_path_then_dir() {
        let name = if cfg!(windows) { "rigctld.exe" } else { "rigctld" };
        let on_path = tempfile::tempdir().unwrap();
        let chosen = tempfile::tempdir().unwrap();
        let empty = tempfile::tempdir().unwrap();
        std::fs::write(on_path.path().join(name), b"").unwrap();
        std::fs::write(chosen.path().join(name), b"").unwrap();
        let path_var = std::env::join_paths([empty.path(), on_path.path()]).unwrap();
        assert_eq!(find_rigctld_in(&path_var, Some(chosen.path())), Some(on_path.path().join(name)));
        let only_empty = std::env::join_paths([empty.path()]).unwrap();
        assert_eq!(find_rigctld_in(&only_empty, Some(chosen.path())), Some(chosen.path().join(name)));
        assert_eq!(find_rigctld_in(&only_empty, None), None);
    }
}
