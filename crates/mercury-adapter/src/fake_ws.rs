//! A fake Mercury telemetry WebSocket for tests (feature `fake`): a plain
//! `ws://` server on an ephemeral loopback port. Like Mercury, it repeats the
//! current `status` every 500 ms to every client; tests add one-off frames
//! (garbage included), drop every connection, or stop the server outright by
//! dropping it. It never reads what a client sends.

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

#[derive(Default)]
struct Shared {
    status: Option<String>,
    accepted: usize,
    conns: Vec<JoinHandle<()>>,
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
    let mut tick = tokio::time::interval(STATUS_EVERY);
    loop {
        let msg = tokio::select! {
            _ = tick.tick() => match shared.lock().unwrap().status.clone() {
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
                Some(Ok(_)) => continue,
                _ => return,
            },
        };
        if tx.send(msg).await.is_err() {
            return;
        }
    }
}
