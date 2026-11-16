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
    bcast_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Listen mode set by `MODE n` (None: Mercury's default, 1).
    mode: Option<u8>,
    monitor: bool,
    mode_wrong: bool,
}

impl Station {
    fn listen_mode(&self) -> u8 {
        self.mode.unwrap_or(1)
    }
}

#[derive(Default)]
struct Shared {
    st: [Station; 2],
    session: Option<(String, String)>,
    hold: bool,
    /// Hold only what station `i` sends.
    hold_from: [bool; 2],
    /// A CONNECT nobody answers stays pending (Mercury keeps retrying).
    unanswered_calls: bool,
    held: Vec<(usize, Vec<u8>)>,
    /// Bytes delivered to the far data socket, per direction (0: A->B, 1: B->A).
    delivered: [u64; 2],
    log: Vec<String>,
    /// Broadcasts put on the air, as (sender, payload after truncation).
    bcast_log: Vec<(usize, Vec<u8>)>,
}

pub struct FakeNet {
    pub a_base: u16,
    pub b_base: u16,
    /// Broadcast (KISS) ports, one client each, as Mercury.
    pub a_bcast: u16,
    pub b_bcast: u16,
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
        let (a_bl, b_bl) =
            (TcpListener::bind(("127.0.0.1", 0)).await.unwrap(), TcpListener::bind(("127.0.0.1", 0)).await.unwrap());
        let net = Arc::new(FakeNet {
            a_base,
            b_base,
            a_bcast: a_bl.local_addr().unwrap().port(),
            b_bcast: b_bl.local_addr().unwrap().port(),
            shared: Arc::default(),
            kick: [Arc::new(Notify::new()), Arc::new(Notify::new())],
            connect_delay: Duration::from_millis(50),
        });
        for (i, (c, d)) in [(ac, ad), (bc, bd)].into_iter().enumerate() {
            tokio::spawn(control_accept(net.clone(), i, c));
            tokio::spawn(data_accept(net.clone(), i, d));
        }
        tokio::spawn(bcast_accept(net.clone(), 0, a_bl));
        tokio::spawn(bcast_accept(net.clone(), 1, b_bl));
        net
    }

    /// Stop forwarding data; bytes accumulate as "in flight".
    pub fn set_hold(&self, hold: bool) {
        self.shared.lock().unwrap().hold = hold;
    }

    /// Hold only what station `from` sends (its modem keeps it).
    pub fn set_hold_from(&self, from: usize, hold: bool) {
        self.shared.lock().unwrap().hold_from[from] = hold;
    }

    /// How many writes are held.
    pub fn held_len(&self) -> usize {
        self.shared.lock().unwrap().held.len()
    }

    /// Remove and return the held writes, as (sender, bytes).
    pub fn take_held(&self) -> Vec<(usize, Vec<u8>)> {
        std::mem::take(&mut self.shared.lock().unwrap().held)
    }

    /// Deliver `bytes` as if station `from` had just sent them.
    pub fn inject(&self, from: usize, bytes: Vec<u8>) {
        deliver(&mut self.shared.lock().unwrap(), from, bytes);
    }

    /// Send a raw control line to station `to` (fault injection).
    pub fn send_control(&self, to: usize, line: &str) {
        send_line(&self.shared.lock().unwrap(), to, line);
    }

    /// Bring a link up as Mercury would, but with a CONNECTED line of the
    /// test's choosing, sent to station `to` only.
    pub fn raw_connected(&self, to: usize, line: &str) {
        let mut s = self.shared.lock().unwrap();
        s.session = Some(("?".into(), "?".into()));
        send_line(&s, to, line);
    }

    /// Calls to an unreachable station stay pending instead of failing.
    pub fn set_unanswered_calls(&self, on: bool) {
        self.shared.lock().unwrap().unanswered_calls = on;
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
            s.st[i].bcast_tx = None;
        }
        self.kick[i].notify_waiters();
    }

    /// Make station `i` answer `MODE n` with WRONG (an unsupported mode).
    pub fn set_mode_wrong(&self, i: usize, wrong: bool) {
        self.shared.lock().unwrap().st[i].mode_wrong = wrong;
    }

    /// Broadcasts put on the air, as (sender, payload as transmitted). Dropped
    /// ones (session up) are not listed; over-capacity ones are truncated.
    pub fn broadcast_log(&self) -> Vec<(usize, Vec<u8>)> {
        self.shared.lock().unwrap().bcast_log.clone()
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
        ["MODE", n] => {
            let n = n.parse::<u8>();
            if s.session.is_some() {
                send_line(&s, i, "BUSY");
            } else if s.st[i].mode_wrong || n.is_err() {
                send_line(&s, i, "WRONG");
            } else {
                s.st[i].mode = n.ok();
                send_line(&s, i, "OK");
            }
        }
        ["MONITOR", v @ ("ON" | "OFF")] => {
            s.st[i].monitor = *v == "ON";
            send_line(&s, i, "OK");
        }
        ["CQFRAME", c, bw] => {
            send_line(&s, i, "OK");
            let (c, bw) = ((*c).to_string(), (*bw).to_string());
            tokio::spawn(transmit_cq(net.clone(), i, c, bw));
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
                } else if !s.unanswered_calls {
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
        if s.hold || s.hold_from[i] {
            s.held.push((i, bytes));
        } else {
            deliver(&mut s, i, bytes);
            send_line(&s, i, "BUFFER 0");
        }
    }
    writer.abort();
    net.shared.lock().unwrap().st[i].data_tx = None;
}

/// One CQ on the air: PENDING, PTT window, delivery to the other side.
async fn transmit_cq(net: Arc<FakeNet>, i: usize, call: String, bw: String) {
    let gap = || tokio::time::sleep(Duration::from_millis(50));
    gap().await;
    send_line(&net.shared.lock().unwrap(), i, "PENDING");
    gap().await;
    send_line(&net.shared.lock().unwrap(), i, "PTT ON");
    gap().await;
    gap().await;
    {
        let s = net.shared.lock().unwrap();
        let j = 1 - i;
        send_line(&s, j, &format!("CQFRAME {call} {bw}"));
        if s.st[j].monitor {
            send_line(&s, j, &format!("MONITOR DATAC16 CQ FROM={call} BW={bw} SNR=7.5"));
        }
        send_line(&s, i, "PTT OFF");
    }
    gap().await;
    send_line(&net.shared.lock().unwrap(), i, "CANCELPENDING");
}

/// A KISS frame from station `i`'s host. Mercury's measured behaviour: dropped
/// while a session is up; silently cut to the mode's wrapped capacity;
/// heard only by a station on the same listen mode.
async fn transmit_broadcast(net: Arc<FakeNet>, i: usize, mut payload: Vec<u8>) {
    let mode = {
        let mut s = net.shared.lock().unwrap();
        if s.session.is_some() {
            return;
        }
        let mode = s.st[i].listen_mode();
        if let Some(cap) = crate::kiss::wrapped_capacity(mode) {
            payload.truncate(cap);
        }
        s.bcast_log.push((i, payload.clone()));
        send_line(&s, i, "PTT ON");
        mode
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let s = net.shared.lock().unwrap();
    let j = 1 - i;
    if s.st[j].listen_mode() == mode {
        if let Some(tx) = &s.st[j].bcast_tx {
            let _ = tx.send(crate::kiss::encode(crate::kiss::CMD_DATA_STD, &payload));
        }
    }
    send_line(&s, i, "PTT OFF");
}

async fn bcast_accept(net: Arc<FakeNet>, i: usize, l: TcpListener) {
    loop {
        let Ok((sock, _)) = l.accept().await else { return };
        if net.shared.lock().unwrap().st[i].bcast_tx.is_some() {
            drop(sock); // one-client rule: newcomer closed at once
            continue;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        net.shared.lock().unwrap().st[i].bcast_tx = Some(tx);
        tokio::spawn(bcast_conn(net.clone(), i, sock, rx));
    }
}

async fn bcast_conn(net: Arc<FakeNet>, i: usize, sock: TcpStream, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    let (mut r, mut w) = sock.into_split();
    let kick = net.kick[i].clone();
    let writer = tokio::spawn(async move {
        while let Some(b) = rx.recv().await {
            if w.write_all(&b).await.is_err() {
                break;
            }
        }
    });
    let mut dec = crate::kiss::Decoder::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = tokio::select! {
            r = r.read(&mut buf) => match r { Ok(0) | Err(_) => break, Ok(n) => n },
            _ = kick.notified() => break,
        };
        for f in dec.push(&buf[..n]) {
            if f.cmd == crate::kiss::CMD_DATA_STD {
                tokio::spawn(transmit_broadcast(net.clone(), i, f.payload));
            }
        }
    }
    writer.abort();
    net.shared.lock().unwrap().st[i].bcast_tx = None;
}
