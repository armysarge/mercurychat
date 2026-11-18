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
        }
    }
}

/// The status as sent: the set one, with `tx_gain_db` as last commanded.
fn status_now(s: &Shared) -> Option<String> {
    let status = s.status.clone()?;
    let Some(g) = s.tx_gain_db else { return Some(status) };
    match serde_json::from_str::<serde_json::Value>(&status) {
        Ok(serde_json::Value::Object(mut o)) => {
            o.insert("tx_gain_db".into(), serde_json::json!(g));
            Some(serde_json::Value::Object(o).to_string())
        }
        _ => Some(status),
    }
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
    let hello = shared.lock().unwrap().radio_list.clone();
    if let Some(rl) = hello {
        if tx.send(Message::text(rl)).await.is_err() {
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
                    let mut s = shared.lock().unwrap();
                    s.received.push(t.clone());
                    let cmd: Option<serde_json::Value> = serde_json::from_str(&t).ok();
                    let gain = cmd
                        .as_ref()
                        .filter(|c| c["command"] == "set_tx_gain")
                        .and_then(|c| c["value"].as_str()?.parse::<f64>().ok());
                    match gain {
                        Some(g) if s.answer_commands => {
                            s.tx_gain_db = Some(g);
                            Message::text(r#"{"status":"ok"}"#)
                        }
                        _ => continue,
                    }
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
