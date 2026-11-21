//! The Mercury contract harness (M3c design A1-A4).
//!
//! A contract is written once against `mercury-adapter`'s public API and run
//! on one of two targets, chosen by `MC_CONTRACT_TARGET`:
//!
//! - `fake` (default): a fresh `FakeNet` per contract, as `scripts/check.sh`
//!   runs it on every check;
//! - `real`: two real Mercury stations in the Docker lab
//!   (`lab/docker/contract/run-contracts.sh`), whose endpoints come from
//!   `MC_A_BASE`, `MC_B_BASE`, `MC_A_BCAST`, `MC_B_BCAST`, `MC_A_WS`,
//!   `MC_B_WS`, `MC_A_CALL` and `MC_B_CALL`.
//!
//! Each contract prints `CONTRACT <name> PASS|SKIP|DIVERGENCE|FAIL`, which the
//! lab's summary is built from. A contract listed in the divergence file
//! (`MC_DIVERGENCES`, default `lab/docker/contract/divergences.txt`) that
//! fails on the real target is reported as a divergence instead of failing;
//! one that passes there fails with `DIVERGENCE-FIXED`, so the list stays
//! honest. On the fake every contract must pass: the fake is never bent to a
//! listed divergence (A4).

use mercury_adapter::fake::FakeNet;
use mercury_adapter::{
    Ack, AdapterEvent, Attachment, Callsign, Command, ControlEvent, Endpoint, MercuryClient, Timeouts,
};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Fake,
    Real,
}

/// Where a contract can run (A3): fake-only cases can only be provoked on the
/// fake; real-only ones depend on real timing or on what only Mercury sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Both,
    FakeOnly,
    RealOnly,
}

/// One station of the pair.
#[derive(Debug, Clone)]
pub struct Station {
    pub ep: Endpoint,
    /// Mercury's broadcast (KISS) port.
    pub bcast: SocketAddr,
    /// Mercury's UI WebSocket, when the target has one.
    pub ws: Option<String>,
    pub call: Callsign,
}

/// Two stations, A and B, and how long things may take on this target.
pub struct Target {
    pub kind: TargetKind,
    pub a: Station,
    pub b: Station,
    pub t: Timeouts,
    fake: Option<Arc<FakeNet>>,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set for the real target"))
}

fn port(name: &str) -> u16 {
    env(name).parse().unwrap_or_else(|_| panic!("{name} is not a port"))
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

impl Target {
    pub fn kind() -> TargetKind {
        match std::env::var("MC_CONTRACT_TARGET").as_deref() {
            Ok("real") => TargetKind::Real,
            Ok("fake") | Err(_) => TargetKind::Fake,
            Ok(other) => panic!("MC_CONTRACT_TARGET={other}: use fake or real"),
        }
    }

    pub async fn get() -> Target {
        match Target::kind() {
            TargetKind::Fake => {
                let net = FakeNet::start().await;
                let st = |base: u16, bcast: u16, call: &str| Station {
                    ep: Endpoint::base("127.0.0.1", base),
                    bcast: loopback(bcast),
                    ws: None,
                    call: Callsign::parse(call).unwrap(),
                };
                Target {
                    kind: TargetKind::Fake,
                    a: st(net.a_base, net.a_bcast, "N0CTA"),
                    b: st(net.b_base, net.b_bcast, "N0CTB"),
                    t: Timeouts::default(),
                    fake: Some(net),
                }
            }
            TargetKind::Real => {
                let st = |side: &str| Station {
                    ep: Endpoint::base("127.0.0.1", port(&format!("MC_{side}_BASE"))),
                    bcast: loopback(port(&format!("MC_{side}_BCAST"))),
                    ws: std::env::var(format!("MC_{side}_WS")).ok(),
                    call: Callsign::parse(&env(&format!("MC_{side}_CALL"))).expect("a valid callsign"),
                };
                let t = Timeouts { command_ack: Duration::from_secs(10), ..Timeouts::default() };
                Target { kind: TargetKind::Real, a: st("A"), b: st("B"), t, fake: None }
            }
        }
    }

    /// The fake's knobs (fake-only contracts, and setting up the fake to behave as Mercury does).
    pub fn fake(&self) -> Option<&FakeNet> {
        self.fake.as_deref()
    }

    /// How long an ARQ connect, a delivery or a disconnect may take.
    pub fn arq_timeout(&self) -> Duration {
        match self.kind {
            TargetKind::Fake => Duration::from_secs(5),
            TargetKind::Real => Duration::from_secs(120),
        }
    }

    /// How long a direct reply to a command or query may take.
    pub fn reply(&self) -> Duration {
        match self.kind {
            TargetKind::Fake => Duration::from_secs(2),
            TargetKind::Real => Duration::from_secs(10),
        }
    }
}

/// Attach to a station, or fail the contract with the reason.
pub async fn attach(st: &Station, t: &Target) -> Attachment {
    match MercuryClient::attach(&st.ep, t.t).await {
        Ok(a) => a,
        Err(e) => panic!("attach to {}:{}: {e}", st.ep.host, st.ep.control_port),
    }
}

/// The next control event that `pred` accepts, within `within`; other
/// events are skipped (and listed if it never comes).
pub async fn next_event(
    a: &mut Attachment,
    within: Duration,
    what: &str,
    pred: impl Fn(&ControlEvent) -> bool,
) -> ControlEvent {
    let end = tokio::time::Instant::now() + within;
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout_at(end, a.events.recv()).await {
            Ok(Some(AdapterEvent::Control(e))) if pred(&e) => return e,
            Ok(Some(AdapterEvent::Control(e))) => seen.push(format!("{e:?}")),
            Ok(Some(AdapterEvent::RawLine(_))) => {}
            Ok(Some(other)) => panic!("waiting for {what}: {other:?}"),
            Ok(None) => panic!("waiting for {what}: the control connection closed"),
            Err(_) => panic!("no {what} within {} s; seen {seen:?}", within.as_secs()),
        }
    }
}

/// A and B attached, B listening, A connected to B (both saw CONNECTED).
pub async fn link(t: &Target) -> (Attachment, Attachment) {
    let mut a = attach(&t.a, t).await;
    let mut b = attach(&t.b, t).await;
    for (x, call) in [(&a, &t.a.call), (&b, &t.b.call)] {
        assert_eq!(x.client.command(&Command::MyCall(call.clone())).await, Ok(Ack::Ok));
    }
    assert_eq!(b.client.command(&Command::Listen(true)).await, Ok(Ack::Ok));
    let connect = Command::Connect { mycall: t.a.call.clone(), theircall: t.b.call.clone() };
    assert_eq!(a.client.command(&connect).await, Ok(Ack::Ok));
    let up = |e: &ControlEvent| matches!(e, ControlEvent::Connected { .. });
    next_event(&mut a, t.arq_timeout(), "CONNECTED on A", up).await;
    next_event(&mut b, t.arq_timeout(), "CONNECTED on B", up).await;
    (a, b)
}

/// Real Mercury keeps a session or a listen across clients: end both before
/// each contract, so one contract never runs into the last one's state.
async fn reset_real(t: &Target) {
    for st in [&t.a, &t.b] {
        if let Ok(a) = MercuryClient::attach(&st.ep, t.t).await {
            let _ = a.client.command(&Command::Abort).await;
            let _ = a.client.command(&Command::Listen(false)).await;
        }
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
}

fn divergences() -> Vec<String> {
    let path = std::env::var("MC_DIVERGENCES")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../lab/docker/contract/divergences.txt").into());
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Run one contract (see the module docs for what is printed and when it fails).
pub async fn contract<F, Fut>(name: &str, tag: Tag, body: F)
where
    F: FnOnce(Target) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let kind = Target::kind();
    if matches!((tag, kind), (Tag::FakeOnly, TargetKind::Real) | (Tag::RealOnly, TargetKind::Fake)) {
        println!("CONTRACT {name} SKIP");
        return;
    }
    let t = Target::get().await;
    if kind == TargetKind::Real {
        reset_real(&t).await;
    }
    let listed = kind == TargetKind::Real && divergences().iter().any(|d| d == name);
    match (tokio::spawn(body(t)).await, listed) {
        (Ok(()), false) => println!("CONTRACT {name} PASS"),
        (Ok(()), true) => {
            println!("DIVERGENCE-FIXED {name}");
            panic!("{name} is listed as a divergence but passes on real Mercury: take it off the list");
        }
        (Err(e), true) => println!("CONTRACT {name} DIVERGENCE ({e})"),
        (Err(e), false) => {
            println!("CONTRACT {name} FAIL");
            match e.try_into_panic() {
                Ok(p) => std::panic::resume_unwind(p),
                Err(e) => panic!("{name}: {e}"),
            }
        }
    }
}
