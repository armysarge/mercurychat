//! M0 broadcast/discovery scenario (docs/03 §6, DSC-01..04) against two real
//! Mercury modems: CQFRAME both ways, MCB/1 beacons as wrapped KISS messages
//! on a shared listen mode, the capacity guard, foreign and malformed traffic,
//! the no-broadcast-during-session gate, BUSY detection and MONITOR lines.
//!
//! The gate and capacity rules here are the ones MercuryChat must apply in M3:
//! Mercury silently truncates over-capacity broadcasts and silently drops a
//! broadcast submitted during an ARQ session (lab probe, 15 Nov 2026).

use crate::scenario::{Report, StepResult};
use mch_protocol::beacon::{self, Beacon, Received};
use mercury_adapter::kiss::{self, KissFrame};
use mercury_adapter::{AdapterEvent, Bandwidth, Callsign, Command, ControlEvent, Endpoint, MercuryClient, Timeouts};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;

/// Listen mode MercuryChat standardises on for MCB/1 (Mercury's default, DATAC3).
pub const BEACON_LISTEN_MODE: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum GateRefusal {
    /// Calling, connected or draining: Mercury would accept and then drop it.
    SessionActive,
    /// Larger than the listen mode's wrapped capacity: Mercury would truncate it.
    TooLarge {
        len: usize,
        capacity: usize,
    },
    UnknownListenMode,
}

type Log<T> = Arc<Mutex<Vec<(Instant, T)>>>;

pub struct Side {
    pub name: &'static str,
    pub call: Callsign,
    client: MercuryClient,
    _data: TcpStream,
    bcast: AsyncMutex<OwnedWriteHalf>,
    events: Log<ControlEvent>,
    lines: Log<String>,
    frames: Log<KissFrame>,
    session: Arc<Mutex<bool>>,
    pub listen_mode: u8,
}

impl Side {
    pub async fn attach(name: &'static str, call: Callsign, ep: &Endpoint, bcast_port: u16) -> anyhow::Result<Side> {
        let att = MercuryClient::attach(ep, Timeouts::default()).await?;
        let bs = TcpStream::connect((ep.host.as_str(), bcast_port)).await?;
        let (mut br, bw) = bs.into_split();
        let events: Log<ControlEvent> = Arc::default();
        let lines: Log<String> = Arc::default();
        let frames: Log<KissFrame> = Arc::default();
        let session = Arc::new(Mutex::new(false));
        {
            let (events, lines, session) = (events.clone(), lines.clone(), session.clone());
            let mut rx = att.events;
            tokio::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    match ev {
                        AdapterEvent::Control(c) => {
                            match &c {
                                ControlEvent::Connected { .. } | ControlEvent::Pending => {
                                    *session.lock().unwrap() = true
                                }
                                ControlEvent::Disconnected | ControlEvent::CancelPending => {
                                    *session.lock().unwrap() = false
                                }
                                _ => {}
                            }
                            events.lock().unwrap().push((Instant::now(), c));
                        }
                        AdapterEvent::RawLine(l) => lines.lock().unwrap().push((Instant::now(), l)),
                        _ => break,
                    }
                }
            });
        }
        {
            let frames = frames.clone();
            tokio::spawn(async move {
                let mut d = kiss::Decoder::new();
                let mut buf = [0u8; 4096];
                while let Ok(n) = br.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    for f in d.push(&buf[..n]) {
                        frames.lock().unwrap().push((Instant::now(), f));
                    }
                }
            });
        }
        Ok(Side {
            name,
            call,
            client: att.client,
            _data: att.data,
            bcast: AsyncMutex::new(bw),
            events,
            lines,
            frames,
            session,
            listen_mode: BEACON_LISTEN_MODE,
        })
    }

    /// Calling counts as an active session from the moment CONNECT is issued.
    pub async fn connect(&self, to: &Callsign) -> String {
        *self.session.lock().unwrap() = true;
        self.cmd(Command::Connect { mycall: self.call.clone(), theircall: to.clone() }).await
    }

    pub async fn cmd(&self, c: Command) -> String {
        format!("{:?}", self.client.command(&c).await)
    }

    /// MercuryChat's broadcast gate (DSC-02/03): idle only, never over capacity.
    pub fn gate(&self, len: usize) -> Result<(), GateRefusal> {
        if *self.session.lock().unwrap() {
            return Err(GateRefusal::SessionActive);
        }
        let capacity = kiss::wrapped_capacity(self.listen_mode).ok_or(GateRefusal::UnknownListenMode)?;
        if len > capacity {
            return Err(GateRefusal::TooLarge { len, capacity });
        }
        Ok(())
    }

    pub async fn broadcast(&self, payload: &[u8]) -> Result<(), GateRefusal> {
        self.gate(payload.len())?;
        let mut w = self.bcast.lock().await;
        w.write_all(&kiss::encode(kiss::CMD_DATA_STD, payload)).await.map_err(|_| GateRefusal::UnknownListenMode)?;
        Ok(())
    }

    pub async fn wait_event(
        &self,
        since: Instant,
        timeout: Duration,
        pred: impl Fn(&ControlEvent) -> bool,
    ) -> Option<Instant> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(t) = self.events.lock().unwrap().iter().find(|(t, e)| *t >= since && pred(e)).map(|(t, _)| *t) {
                return Some(t);
            }
            if Instant::now() >= end {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn wait_frame(&self, since: Instant, timeout: Duration) -> Option<(Instant, KissFrame)> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(f) = self.frames.lock().unwrap().iter().find(|(t, _)| *t >= since).cloned() {
                return Some(f);
            }
            if Instant::now() >= end {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn lines_since(&self, since: Instant, prefix: &str) -> Vec<String> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, l)| *t >= since && l.starts_with(prefix))
            .map(|(_, l)| l.clone())
            .collect()
    }

    pub fn transcript(&self, start: Instant) -> String {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .map(|(t, l)| format!("{:>9} {}\n", t.duration_since(start).as_millis(), l))
            .collect()
    }
}

fn secs(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f64())
}

struct Steps {
    steps: Vec<StepResult>,
    measurements: Vec<(String, String)>,
}

impl Steps {
    fn push(&mut self, step: &str, pass: bool, started: Instant, detail: String) {
        println!(
            "--> {step}\n    {} ({} ms) {detail}",
            if pass { "PASS" } else { "FAIL" },
            started.elapsed().as_millis()
        );
        self.steps.push(StepResult { step: step.into(), pass, millis: started.elapsed().as_millis(), detail });
    }
    fn measure(&mut self, k: &str, v: String) {
        println!("    measure {k} = {v}");
        self.measurements.push((k.into(), v));
    }
}

async fn cq(s: &mut Steps, tx: &Side, rx: &Side) {
    let t = Instant::now();
    let ack = tx.cmd(Command::CqFrame { source: tx.call.clone(), bw: Bandwidth::Bw2300 }).await;
    let src = tx.call.as_str().to_string();
    let heard = rx
        .wait_event(t, Duration::from_secs(25), |e| matches!(e, ControlEvent::CqFrame { source, .. } if *source == src))
        .await;
    let pend = tx.wait_event(t, Duration::from_secs(1), |e| *e == ControlEvent::Pending).await;
    let done = tx.wait_event(t, Duration::from_secs(10), |e| *e == ControlEvent::CancelPending).await;
    let ok = heard.is_some() && pend.is_some() && done.is_some();
    s.push(
        &format!("CQFRAME {}->{} heard by a receive-only station", tx.name, rx.name),
        ok,
        t,
        format!(
            "ack {ack}; PENDING..CANCELPENDING {}; {} reported CQFRAME {} after {}",
            match (pend, done) {
                (Some(p), Some(d)) => secs(d - p),
                _ => "missing".into(),
            },
            rx.name,
            src,
            heard.map(|h| secs(h - t)).unwrap_or_else(|| "never".into())
        ),
    );
    if let Some(h) = heard {
        s.measure(&format!("cq_latency_ms[{}->{}]", tx.name, rx.name), (h - t).as_millis().to_string());
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
}

async fn beacon_exchange(s: &mut Steps, tx: &Side, rx: &Side, grid: &str, seq: u32) {
    let t = Instant::now();
    let b = Beacon::new(seq, tx.call.as_str(), Some(grid), beacon::CAP_CHAT | beacon::CAP_FILE).unwrap();
    let p = b.encode();
    let sent = tx.broadcast(&p).await;
    let got = rx.wait_frame(t, Duration::from_secs(30)).await;
    let class = got.as_ref().map(|(_, f)| (f.cmd, beacon::classify(&f.payload)));
    let ok = sent.is_ok() && matches!(&class, Some((0, Received::Beacon(r))) if *r == b);
    s.push(
        &format!("MCB/1 beacon {}->{} (wrapped KISS 0x00, listen mode DATAC3)", tx.name, rx.name),
        ok,
        t,
        format!(
            "{} bytes, gate {:?}; {} got {:?} after {}",
            p.len(),
            sent,
            rx.name,
            class,
            got.map(|(g, _)| secs(g - t)).unwrap_or_else(|| "never".into())
        ),
    );
    if let Some((g, _)) = rx.frames.lock().unwrap().iter().find(|(tt, _)| *tt >= t) {
        s.measure(&format!("beacon_latency_ms[{}->{}]", tx.name, rx.name), (*g - t).as_millis().to_string());
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
}

pub async fn run(a: &mut Side, b: &mut Side) -> Report {
    let mut s = Steps { steps: vec![], measurements: vec![] };

    let t = Instant::now();
    let mut acks = vec![];
    for side in [&*a, &*b] {
        for c in [
            Command::MyCall(side.call.clone()),
            Command::Public(false),
            Command::Listen(false),
            Command::Bw(Bandwidth::Bw2300),
            Command::Mode(BEACON_LISTEN_MODE),
        ] {
            acks.push(format!("{}:{}={}", side.name, c.wire().trim_end(), side.cmd(c).await));
        }
    }
    let all_ok = acks.iter().all(|a| a.ends_with("Ok(Ok)"));
    s.push("attach both, LISTEN OFF (receive-only), MODE 1 (DATAC3)", all_ok, t, acks.join("; "));
    tokio::time::sleep(Duration::from_secs(1)).await;

    cq(&mut s, a, b).await;
    cq(&mut s, b, a).await;
    beacon_exchange(&mut s, a, b, "JJ00aa", 0x4D43_0001).await;
    beacon_exchange(&mut s, b, a, "JJ00", 0x4D43_0002).await;

    // Capacity: 123 bytes (DATAC3 limit) arrives intact and is classified foreign;
    // 124 bytes is refused by the gate and nothing is transmitted.
    let t = Instant::now();
    let cap = kiss::wrapped_capacity(BEACON_LISTEN_MODE).unwrap();
    let full: Vec<u8> = (0..cap).map(|i| (i * 31 % 251) as u8 + 1).collect();
    let sent = a.broadcast(&full).await;
    let got = b.wait_frame(t, Duration::from_secs(30)).await;
    let intact = matches!(&got, Some((_, f)) if f.payload == full);
    let class = got.as_ref().map(|(_, f)| beacon::classify(&f.payload));
    s.push(
        "largest wrapped payload (123 B on DATAC3) arrives intact; non-MCB traffic is Foreign",
        sent.is_ok() && intact && class == Some(Received::Foreign),
        t,
        format!("gate {sent:?}; intact={intact}; classified {class:?}"),
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    let t = Instant::now();
    let over = vec![0x55u8; cap + 1];
    let refused = a.broadcast(&over).await;
    let keyed = a.wait_event(t, Duration::from_secs(6), |e| *e == ControlEvent::Ptt(true)).await;
    s.push(
        "over-capacity payload (124 B) refused before Mercury can truncate it",
        refused == Err(GateRefusal::TooLarge { len: cap + 1, capacity: cap }) && keyed.is_none(),
        t,
        format!("gate {refused:?}; A keyed within 6 s: {}", keyed.is_some()),
    );

    // A truncated MCB1 (what Mercury's silent truncation would deliver) is Malformed, never a station.
    let t = Instant::now();
    let good = Beacon::new(0x4D43_0003, a.call.as_str(), Some("JJ00"), beacon::CAP_CHAT).unwrap().encode();
    let cut = &good[..good.len() - 3];
    let sent = a.broadcast(cut).await;
    let got = b.wait_frame(t, Duration::from_secs(30)).await;
    let class = got.as_ref().map(|(_, f)| beacon::classify(&f.payload));
    s.push(
        "truncated MCB/1 is reported Malformed, not as a heard station",
        sent.is_ok() && matches!(class, Some(Received::Malformed(_))),
        t,
        format!("sent {} of {} bytes; B classified {class:?}", cut.len(), good.len()),
    );
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Session gate: no broadcast while calling/connected; allowed again after DISCONNECTED.
    let t = Instant::now();
    b.cmd(Command::Listen(true)).await;
    a.connect(&b.call.clone()).await;
    let calling = a.gate(20);
    let conn = a.wait_event(t, Duration::from_secs(90), |e| matches!(e, ControlEvent::Connected { .. })).await;
    let during = a.gate(20);
    let td = Instant::now();
    a.cmd(Command::Disconnect).await;
    let da = a.wait_event(td, Duration::from_secs(120), |e| *e == ControlEvent::Disconnected).await;
    b.wait_event(td, Duration::from_secs(30), |e| *e == ControlEvent::Disconnected).await;
    b.cmd(Command::Listen(false)).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = a.gate(20);
    s.push(
        "broadcast gate: refused while calling and connected, open again after DISCONNECTED",
        calling == Err(GateRefusal::SessionActive)
            && conn.is_some()
            && during == Err(GateRefusal::SessionActive)
            && da.is_some()
            && after.is_ok(),
        t,
        format!(
            "calling {calling:?}; connected after {}; during {during:?}; disconnected after {}; after {after:?}",
            conn.map(|c| secs(c - t)).unwrap_or_else(|| "never".into()),
            da.map(|d| secs(d - td)).unwrap_or_else(|| "never".into())
        ),
    );
    beacon_exchange(&mut s, a, b, "JJ00aa", 0x4D43_0004).await;

    // BUSY detection and MONITOR on the receiver while A calls CQ.
    let t = Instant::now();
    let r1 = b.cmd(Command::BusyDetect(true)).await;
    let r2 = b.cmd(Command::Monitor(true)).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let tc = Instant::now();
    a.cmd(Command::CqFrame { source: a.call.clone(), bw: Bandwidth::Bw500 }).await;
    let on = b.wait_event(tc, Duration::from_secs(20), |e| *e == ControlEvent::Busy(true)).await;
    let off = b.wait_event(tc, Duration::from_secs(25), |e| *e == ControlEvent::Busy(false)).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mon = b.lines_since(tc, "MONITOR");
    let src = format!("FROM={}", a.call.as_str());
    b.cmd(Command::Monitor(false)).await;
    b.cmd(Command::BusyDetect(false)).await;
    // BUSY OFF is informational: the detector takes its noise floor from the
    // channel, and a virtual cable is digital silence, so it may never clear
    // here (Mercury TNC.md: the detector needs on-air tuning).
    s.push(
        "BUSY ON at the carrier and MONITOR CQ line (with SNR) on the receiver",
        on.is_some() && mon.iter().any(|l| l.contains(" CQ ") && l.contains(&src) && l.contains("SNR=")),
        t,
        format!(
            "BUSYDETECT {r1}, MONITOR {r2}; BUSY ON after {}, OFF after {}; {mon:?}",
            on.map(|x| secs(x - tc)).unwrap_or_else(|| "never".into()),
            off.map(|x| secs(x - tc)).unwrap_or_else(|| "never".into())
        ),
    );

    let failed = s.steps.iter().filter(|x| !x.pass).count();
    Report {
        mode: "M0 broadcast/discovery over two Mercury modems".into(),
        passed: s.steps.len() - failed,
        failed,
        steps: s.steps,
        measurements: s.measurements,
    }
}
