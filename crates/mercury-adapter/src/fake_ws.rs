//! A fake Mercury telemetry WebSocket for tests (feature `fake`): a plain
//! `ws://` server on an ephemeral loopback port. Like Mercury, it repeats the
//! current `status` every 500 ms to every client; tests add one-off frames
//! (garbage included), drop every connection, or stop the server outright by
//! dropping it. It records what clients send, answers `set_tx_gain` as
//! Mercury does (and reflects it in the next status) unless told not to, and
//! sends a `radio_list` to each new client when one is set.

use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// Mercury's status cadence (`UI_PUBLISH_INTERVAL_US`).
pub const STATUS_EVERY: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
enum Out {
    Text(String),
    Binary(Vec<u8>),
    Kill,
}

struct Shared {
    status: Option<String>,
    accepted: usize,
    conns: Vec<JoinHandle<()>>,
    radio_list: Option<String>,
    received: Vec<String>,
    tx_gain_db: Option<f64>,
    answer_commands: bool,
    /// Frames sent to each client after the radio_list (device lists).
    hello: Vec<String>,
    /// What `set_audio_config` brings: audio_ok and audio_error in the status.
    audio_result: (bool, String),
    /// Set once a `set_audio_config` has been carried out.
    audio_now: Option<(bool, String)>,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            status: None,
            accepted: 0,
            conns: vec![],
            radio_list: None,
            received: vec![],
            tx_gain_db: None,
            answer_commands: true,
            hello: vec![],
            audio_result: (true, String::new()),
            audio_now: None,
        }
    }
}

/// The status as sent: the set one, with `tx_gain_db` as last commanded.
fn status_now(s: &Shared) -> Option<String> {
    let status = s.status.clone()?;
    if s.tx_gain_db.is_none() && s.audio_now.is_none() {
        return Some(status);
    }
    match serde_json::from_str::<serde_json::Value>(&status) {
        Ok(serde_json::Value::Object(mut o)) => {
            if let Some(g) = s.tx_gain_db {
                o.insert("tx_gain_db".into(), serde_json::json!(g));
            }
            if let Some((ok, err)) = &s.audio_now {
                o.insert("audio_ok".into(), serde_json::json!(ok));
                o.insert("audio_error".into(), serde_json::json!(err));
            }
            Some(serde_json::Value::Object(o).to_string())
        }
        _ => Some(status),
    }
}

/// A client's command, recorded and carried out as Mercury would; the frames
/// to send back.
fn answer_command(s: &mut Shared, text: &str) -> Vec<String> {
    s.received.push(text.to_string());
    let Some(cmd) = serde_json::from_str::<serde_json::Value>(text).ok() else { return vec![] };
    if !s.answer_commands {
        return vec![];
    }
    match cmd["command"].as_str() {
        Some("set_tx_gain") => match cmd["value"].as_str().and_then(|v| v.parse::<f64>().ok()) {
            Some(g) => {
                s.tx_gain_db = Some(g);
                vec![r#"{"status":"ok"}"#.to_string()]
            }
            None => vec![],
        },
        Some("set_audio_config") => {
            s.audio_now = Some(s.audio_result.clone());
            audio_lists_after(s, &cmd)
        }
        _ => vec![],
    }
}

/// The hello frames after a `set_audio_config`: Mercury republishes its
/// device lists with the new selection (only when the restart was healthy).
fn audio_lists_after(s: &Shared, cmd: &serde_json::Value) -> Vec<String> {
    s.hello
        .iter()
        .filter_map(|h| {
            let mut v: serde_json::Value = serde_json::from_str(h).ok()?;
            let key = match v["type"].as_str()? {
                "capture_dev_list" => "value",
                "playback_dev_list" => "value2",
                _ => return Some(h.clone()),
            };
            if s.audio_result.0 {
                v["selected"] = cmd[key].clone();
            }
            Some(v.to_string())
        })
        .collect()
}

pub struct FakeWs {
    /// `ws://127.0.0.1:{port}/websocket`
    pub url: String,
    shared: Arc<Mutex<Shared>>,
    out: broadcast::Sender<Out>,
    accept: JoinHandle<()>,
}

impl FakeWs {
    pub async fn start() -> FakeWs {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind a loopback port");
        let port = listener.local_addr().expect("a local address").port();
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let (out, _) = broadcast::channel(64);
        let accept = {
            let shared = shared.clone();
            let out = out.clone();
            tokio::spawn(async move {
                while let Ok((tcp, _)) = listener.accept().await {
                    let rx = out.subscribe();
                    let task = tokio::spawn(serve(tcp, rx, shared.clone()));
                    let mut s = shared.lock().unwrap();
                    s.conns.retain(|t| !t.is_finished());
                    s.conns.push(task);
                }
            })
        };
        FakeWs { url: format!("ws://127.0.0.1:{port}/websocket"), shared, out, accept }
    }

    /// The status repeated every 500 ms (`None`: send none).
    pub fn set_status(&self, json: Option<&str>) {
        self.shared.lock().unwrap().status = json.map(String::from);
    }

    /// One text frame to every connected client.
    pub fn send_text(&self, text: String) {
        let _ = self.out.send(Out::Text(text));
    }

    /// One binary frame to every connected client.
    pub fn send_binary(&self, bytes: Vec<u8>) {
        let _ = self.out.send(Out::Binary(bytes));
    }

    /// Drop every open connection without a close frame, as a crash would.
    /// The server keeps accepting new ones.
    pub fn kill(&self) {
        let _ = self.out.send(Out::Kill);
    }

    /// The `radio_list` sent to each client as it connects (`None`: none).
    pub fn set_radio_list(&self, json: Option<&str>) {
        self.shared.lock().unwrap().radio_list = json.map(String::from);
    }

    /// Frames sent to each new client after the radio_list: Mercury's
    /// device lists, audio system and input channel.
    pub fn set_hello(&self, frames: Vec<String>) {
        self.shared.lock().unwrap().hello = frames;
    }

    /// What a `set_audio_config` brings (default: healthy).
    pub fn set_audio_result(&self, ok: bool, error: &str) {
        self.shared.lock().unwrap().audio_result = (ok, error.to_string());
    }

    /// Whether `set_tx_gain` is carried out and answered (default yes).
    pub fn set_answer_commands(&self, on: bool) {
        self.shared.lock().unwrap().answer_commands = on;
    }

    /// Every text frame clients have sent, oldest first.
    pub fn received(&self) -> Vec<String> {
        self.shared.lock().unwrap().received.clone()
    }

    /// WebSocket handshakes completed so far.
    pub fn accepted(&self) -> usize {
        self.shared.lock().unwrap().accepted
    }
}

impl Drop for FakeWs {
    /// The server goes away: its port refuses, its connections drop.
    fn drop(&mut self) {
        self.accept.abort();
        let _ = self.out.send(Out::Kill);
        for t in self.shared.lock().unwrap().conns.drain(..) {
            t.abort();
        }
    }
}

async fn serve(tcp: tokio::net::TcpStream, mut rx: broadcast::Receiver<Out>, shared: Arc<Mutex<Shared>>) {
    let Ok(ws) = tokio_tungstenite::accept_async(tcp).await else { return };
    shared.lock().unwrap().accepted += 1;
    let (mut tx, mut incoming) = ws.split();
    let hello = {
        let s = shared.lock().unwrap();
        s.radio_list.iter().chain(s.hello.iter()).cloned().collect::<Vec<_>>()
    };
    for frame in hello {
        if tx.send(Message::text(frame)).await.is_err() {
            return;
        }
    }
    let mut tick = tokio::time::interval(STATUS_EVERY);
    loop {
        let msg = tokio::select! {
            _ = tick.tick() => match status_now(&shared.lock().unwrap()) {
                Some(s) => Message::text(s),
                None => continue,
            },
            o = rx.recv() => match o {
                Ok(Out::Text(t)) => Message::text(t),
                Ok(Out::Binary(b)) => Message::binary(b),
                Ok(Out::Kill) | Err(broadcast::error::RecvError::Closed) => return,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            },
            m = incoming.next() => match m {
                Some(Ok(Message::Text(t))) => {
                    let t = t.as_str().to_string();
                    // The lock is released before anything is sent.
                    let frames = answer_command(&mut shared.lock().unwrap(), &t);
                    for f in frames {
                        if tx.send(Message::text(f)).await.is_err() {
                            return;
                        }
                    }
                    continue;
                }
                Some(Ok(_)) => continue,
                _ => return,
            },
        };
        if tx.send(msg).await.is_err() {
            return;
        }
    }
}
