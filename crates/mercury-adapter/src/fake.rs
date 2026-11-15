//! Deterministic fake Mercury pair (L0 harness, docs/07 §4).
//!
//! Two fake TNCs on ephemeral loopback ports. CONNECT on one side reaches the
//! other when its MYCALL matches and LISTEN is ON; data written to one data
//! socket is delivered to the other in small random pieces, so every reader
//! sees split TCP reads. Fault injection: hold bytes "in flight", drop the RF
//! link, or kill a station's client sockets. This is NOT a waveform test.

use rand::Rng;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};

#[derive(Default)]
struct Station {
    mycall: Option<String>,
    listen: bool,
    ctrl_tx: Option<mpsc::UnboundedSender<String>>,
    data_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
}

#[derive(Default)]
struct Shared {
    st: [Station; 2],
    session: Option<(String, String)>,
    hold: bool,
    held: Vec<(usize, Vec<u8>)>,
    /// Bytes delivered to the far data socket, per direction (0: A->B, 1: B->A).
    delivered: [u64; 2],
    log: Vec<String>,
}

pub struct FakeNet {
    pub a_base: u16,
    pub b_base: u16,
    shared: Arc<Mutex<Shared>>,
    kick: [Arc<Notify>; 2],
    connect_delay: Duration,
}

/// Reserve a free even/odd port pair base..base+1 on loopback.
async fn bind_pair() -> (TcpListener, TcpListener, u16) {
    loop {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let p = l.local_addr().unwrap().port();
        if p == u16::MAX {
            continue;
        }
        if let Ok(l2) = TcpListener::bind(("127.0.0.1", p + 1)).await {
            return (l, l2, p);
        }
    }
}

impl FakeNet {
    pub async fn start() -> Arc<FakeNet> {
        let (ac, ad, a_base) = bind_pair().await;
        let (bc, bd, b_base) = bind_pair().await;
        let net = Arc::new(FakeNet {
            a_base,
            b_base,
            shared: Arc::default(),
            kick: [Arc::new(Notify::new()), Arc::new(Notify::new())],
            connect_delay: Duration::from_millis(50),
        });
        for (i, (c, d)) in [(ac, ad), (bc, bd)].into_iter().enumerate() {
            tokio::spawn(control_accept(net.clone(), i, c));
            tokio::spawn(data_accept(net.clone(), i, d));
        }
        net
    }

    /// Stop forwarding data; bytes accumulate as "in flight".
    pub fn set_hold(&self, hold: bool) {
        self.shared.lock().unwrap().hold = hold;
    }

    /// Deliver held bytes (to whichever far data socket is attached now).
    pub fn release_held(&self) {
        let mut s = self.shared.lock().unwrap();
        s.hold = false;
        let held = std::mem::take(&mut s.held);
        for (from, bytes) in held {
            deliver(&mut s, from, bytes);
        }
    }

    /// RF link lost: both hosts hear DISCONNECTED, held bytes vanish.
    pub fn drop_link(&self) {
        let mut s = self.shared.lock().unwrap();
        s.held.clear();
        if s.session.take().is_some() {
            for i in 0..2 {
                send_line(&s, i, "DISCONNECTED");
            }
        }
    }

    /// Close station `i`'s client sockets from the modem side. The RF session
    /// is kept, as Mercury keeps it when its host program vanishes.
    pub fn kill_client(&self, i: usize) {
        {
            let mut s = self.shared.lock().unwrap();
            s.st[i].ctrl_tx = None;
            s.st[i].data_tx = None;
        }
        self.kick[i].notify_waiters();
    }

    pub fn session_up(&self) -> bool {
        self.shared.lock().unwrap().session.is_some()
    }

    pub fn delivered(&self) -> [u64; 2] {
        self.shared.lock().unwrap().delivered
    }

    pub fn log(&self) -> Vec<String> {
        self.shared.lock().unwrap().log.clone()
    }
}

fn send_line(s: &Shared, i: usize, line: &str) {
    if let Some(tx) = &s.st[i].ctrl_tx {
        let _ = tx.send(format!("{line}\r"));
    }
}

fn deliver(s: &mut Shared, from: usize, bytes: Vec<u8>) {
    let to = 1 - from;
    s.delivered[from] += bytes.len() as u64;
    if let Some(tx) = &s.st[to].data_tx {
        // Split into random small pieces to exercise reassembly.
        let mut rng = rand::rng();
        let mut i = 0;
        while i < bytes.len() {
            let n = rng.random_range(1..=37).min(bytes.len() - i);
            let _ = tx.send(bytes[i..i + n].to_vec());
            i += n;
        }
    }
}

async fn control_accept(net: Arc<FakeNet>, i: usize, l: TcpListener) {
    loop {
        let Ok((sock, _)) = l.accept().await else { return };
        let occupied = net.shared.lock().unwrap().st[i].ctrl_tx.is_some();
        if occupied {
            drop(sock); // one-client rule: newcomer closed at once
            continue;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut s = net.shared.lock().unwrap();
            s.st[i].ctrl_tx = Some(tx);
            if let Some(c) = s.st[i].mycall.clone() {
                send_line(&s, i, &format!("REGISTERED {c}"));
            }
        }
        tokio::spawn(control_conn(net.clone(), i, sock, rx));
    }
}

async fn control_conn(net: Arc<FakeNet>, i: usize, sock: TcpStream, mut rx: mpsc::UnboundedReceiver<String>) {
    let (mut r, mut w) = sock.into_split();
    let kick = net.kick[i].clone();
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if w.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let mut parser = crate::control::LineParser::new();
    let mut buf = [0u8; 512];
    loop {
        let n = tokio::select! {
            r = r.read(&mut buf) => match r { Ok(0) | Err(_) => break, Ok(n) => n },
            _ = kick.notified() => break,
        };
        let Ok(lines) = parser.push(&buf[..n]) else { break };
        for line in lines {
            handle_command(&net, i, &line).await;
        }
    }
    writer.abort();
    let mut s = net.shared.lock().unwrap();
    s.st[i].ctrl_tx = None;
}

async fn handle_command(net: &Arc<FakeNet>, i: usize, line: &str) {
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut s = net.shared.lock().unwrap();
    s.log.push(format!("{}> {line}", ["A", "B"][i]));
    match words.as_slice() {
        ["MYCALL", c, ..] => {
            s.st[i].mycall = Some((*c).to_string());
            send_line(&s, i, "OK");
            send_line(&s, i, &format!("REGISTERED {c}"));
        }
        ["LISTEN", v] => {
            s.st[i].listen = *v != "OFF";
            send_line(&s, i, "OK");
        }
        ["PUBLIC", _] | ["CHAT", _] | ["COMPRESSION", _] => send_line(&s, i, "OK"),
        [bw] if bw.starts_with("BW") => send_line(&s, i, "OK"),
        ["CONNECT", a, b] => {
            send_line(&s, i, "OK");
            let (a, b) = ((*a).to_string(), (*b).to_string());
            let j = 1 - i;
            let reachable = s.session.is_none() && s.st[j].listen && s.st[j].mycall.as_deref() == Some(&b);
            let net2 = net.clone();
            let delay = net.connect_delay;
            drop(s);
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let mut s = net2.shared.lock().unwrap();
                if reachable && s.session.is_none() {
                    s.session = Some((a.clone(), b.clone()));
                    for k in 0..2 {
                        send_line(&s, k, &format!("CONNECTED {a} {b} 2300"));
                    }
                } else {
                    send_line(&s, i, "DISCONNECTED");
                }
            });
        }
        ["DISCONNECT"] | ["ABORT"] => {
            send_line(&s, i, "OK");
            if words[0] == "ABORT" {
                s.held.clear();
            }
            if s.session.take().is_some() {
                for k in 0..2 {
                    send_line(&s, k, "DISCONNECTED");
                }
                // Like Mercury: the teardown frame is keyed after DISCONNECTED.
                send_line(&s, i, "PTT ON");
                send_line(&s, i, "PTT OFF");
            } else {
                send_line(&s, i, "DISCONNECTED");
            }
        }
        ["VERSION"] => send_line(&s, i, "VARA version 4.9.0 registered"),
        ["BUFFER"] => send_line(&s, i, "BUFFER 0"),
        _ => send_line(&s, i, "WRONG"),
    }
}

async fn data_accept(net: Arc<FakeNet>, i: usize, l: TcpListener) {
    loop {
        let Ok((sock, _)) = l.accept().await else { return };
        let occupied = net.shared.lock().unwrap().st[i].data_tx.is_some();
        if occupied {
            drop(sock);
            continue;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        net.shared.lock().unwrap().st[i].data_tx = Some(tx);
        tokio::spawn(data_conn(net.clone(), i, sock, rx));
    }
}

async fn data_conn(net: Arc<FakeNet>, i: usize, sock: TcpStream, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    let (mut r, mut w) = sock.into_split();
    let kick = net.kick[i].clone();
    let writer = tokio::spawn(async move {
        while let Some(b) = rx.recv().await {
            if w.write_all(&b).await.is_err() {
                break;
            }
        }
    });
    let mut buf = [0u8; 4096];
    loop {
        let n = tokio::select! {
            r = r.read(&mut buf) => match r { Ok(0) | Err(_) => break, Ok(n) => n },
            _ = kick.notified() => break,
        };
        let mut s = net.shared.lock().unwrap();
        if s.session.is_none() {
            continue; // data port carries payload only while CONNECTED
        }
        let bytes = buf[..n].to_vec();
        send_line(&s, i, &format!("BUFFER {n}"));
        if s.hold {
            s.held.push((i, bytes));
        } else {
            deliver(&mut s, i, bytes);
            send_line(&s, i, "BUFFER 0");
        }
    }
    writer.abort();
    net.shared.lock().unwrap().st[i].data_tx = None;
}
