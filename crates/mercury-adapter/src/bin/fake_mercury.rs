//! A stand-in for Mercury in the built-in modem's supervisor tests (M5 Task 4):
//! takes Mercury's flags and serves one `FakeNet` station on `-p` (control,
//! and data on the next port) and `-b` (broadcast), and a `FakeWs` on `-U`.
//! It writes what it was given to the `-L` log.
//!
//! Extra flags come from the command line or from `fake-mercury.args` beside
//! the log (so a test can change one start without changing the supervisor):
//! - `--die-after <ms>`: log `fake-mercury: dying` and exit with code 3.
//! - `--audio-fail <text>`: a `set_audio_config` is answered `audio_ok: false`
//!   with `<text>` as the error.
//! - `--expect-rigctld <port>`: log whether a rigctld answers on that port at
//!   start (`fake-mercury: rigctld answered` / `... not answering`).
//!
//! The UI WebSocket greets with device lists and a healthy status, as Mercury.

use mercury_adapter::fake::FakeNet;
use mercury_adapter::fake_ws::FakeWs;
use std::io::Write;
use std::path::PathBuf;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |args: &[String], f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let log_path = flag(&args, "-L").map(PathBuf::from);
    if let Some(extra) =
        log_path.as_ref().and_then(|l| std::fs::read_to_string(l.with_file_name("fake-mercury.args")).ok())
    {
        args.extend(extra.split_whitespace().map(String::from));
    }
    let mut log = log_path
        .as_ref()
        .map(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).expect("open the -L log"));
    let mut say = |line: &str| {
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{line}");
        }
        eprintln!("{line}");
    };
    say(&format!("fake-mercury: args {}", args.join(" ")));

    let port = |f: &str, default: u16| flag(&args, f).and_then(|v| v.parse::<u16>().ok()).unwrap_or(default);
    let (control, bcast, ui) = (port("-p", 8300), port("-b", 8100), port("-U", 10000));
    let net = match FakeNet::start_at(control, bcast).await {
        Ok(n) => n,
        Err(e) => {
            say(&format!("fake-mercury: cannot listen on {control}/{bcast}: {e}"));
            std::process::exit(2);
        }
    };
    let ws = match FakeWs::start_at(ui).await {
        Ok(w) => w,
        Err(e) => {
            say(&format!("fake-mercury: cannot listen on {ui}: {e}"));
            std::process::exit(2);
        }
    };
    ws.set_status(Some(r#"{"type":"status","audio_ok":true,"audio_error":""}"#));
    ws.set_hello(vec![
        r#"{"type":"capture_dev_list","selected":"in1","list":[{"name":"Line In","id":"in1"},{"name":"USB CODEC","id":"in2"}]}"#.into(),
        r#"{"type":"playback_dev_list","selected":"out1","list":[{"name":"Speakers","id":"out1"},{"name":"USB CODEC","id":"out2"}]}"#.into(),
        r#"{"type":"audio_system","selected":"wasapi","list":["wasapi"]}"#.into(),
        r#"{"type":"input_channel","selected":"left","list":["left","right","stereo"]}"#.into(),
    ]);
    if let Some(text) = flag(&args, "--audio-fail") {
        ws.set_audio_result(false, &text);
    }
    if let Some(port) = flag(&args, "--expect-rigctld").and_then(|p| p.parse::<u16>().ok()) {
        let up = mercury_adapter::rigctl::answers_as_rigctld(([127, 0, 0, 1], port).into()).await;
        say(if up { "fake-mercury: rigctld answered" } else { "fake-mercury: rigctld not answering" });
    }
    say(&format!("fake-mercury: listening control {} broadcast {} ui {}", net.a_base, net.a_bcast, ws.url));

    if let Some(ms) = flag(&args, "--die-after").and_then(|v| v.parse::<u64>().ok()) {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        say("fake-mercury: dying");
        std::process::exit(3);
    }
    std::future::pending::<()>().await;
}
