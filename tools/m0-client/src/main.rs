//! MercuryChat M0 test client.
//!
//!   m0-client selftest --work DIR                 L0: in-process fake Mercury pair
//!   m0-client lab --a HOST:PORT --b HOST:PORT ... L1: two real Mercury instances
//!   m0-client probe --endpoint HOST:PORT          attach and print control lines
//!   m0-client broadcast --a .. --b ..             CQ / MCB1 beacon / BUSY / MONITOR proof
//!   m0-client peer --host H --port P --call SIM2  headless MercuryChat peer (M2 labs)
//!
//! Every run writes report.json, report.md and control transcripts to --work.

#![allow(clippy::too_many_arguments)]
mod broadcast;
mod lab;
mod scenario;

use clap::{Parser, Subcommand};
use lab::Handle;
use mc_core::{Station, StationConfig};
use mercury_adapter::{Bandwidth, Callsign, Endpoint, Timeouts};
use scenario::{Budgets, Faults, Report};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about = "MercuryChat M0 modem-integration test client")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// A headless MercuryChat station on one Mercury: LISTEN ON, answer every
    /// call, run the session, and print one JSON line per stored message
    /// ({"event":"received", utc, peer, id, bytes, sha256}). With --echo, reply
    /// "echo: <text>" to each.
    Peer {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Control port (data = port + 1).
        #[arg(long, default_value_t = 8300)]
        port: u16,
        #[arg(long, default_value = "SIM2")]
        call: String,
        #[arg(long)]
        echo: bool,
        /// The peer's own station data (store, files).
        #[arg(long, default_value = "m2-peer")]
        work: PathBuf,
    },
    /// Run the M0 scenario against an in-process fake Mercury pair (no audio).
    Selftest {
        #[arg(long, default_value = "m0-selftest")]
        work: PathBuf,
    },
    /// Run the M0 scenario against two real Mercury instances.
    Lab {
        /// Station A control endpoint host:base_port (data = base+1).
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        #[arg(long, default_value = "LABA")]
        call_a: String,
        #[arg(long, default_value = "LABB")]
        call_b: String,
        #[arg(long, default_value = "m0-lab")]
        work: PathBuf,
        /// Size of the binary file vectors.
        #[arg(long, default_value_t = 4096)]
        file_bytes: usize,
        #[arg(long, default_value = "2300")]
        bw: String,
    },
    /// CQ, MCB/1 beacon, capacity, session-gate, BUSY and MONITOR proof on two real modems.
    Broadcast {
        /// Station A control endpoint host:base_port (data = base+1).
        #[arg(long)]
        a: String,
        #[arg(long)]
        b: String,
        /// Broadcast (KISS) ports.
        #[arg(long, default_value_t = 8100)]
        bcast_a: u16,
        #[arg(long, default_value_t = 8200)]
        bcast_b: u16,
        #[arg(long, default_value = "LABA")]
        call_a: String,
        #[arg(long, default_value = "LABB")]
        call_b: String,
        #[arg(long, default_value = "m0-broadcast")]
        work: PathBuf,
    },
    /// Attach to one Mercury and print control traffic (read-only: LISTEN OFF).
    Probe {
        #[arg(long)]
        endpoint: String,
        #[arg(long, default_value = "PROBE")]
        call: String,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
}

fn endpoint(s: &str) -> anyhow::Result<Endpoint> {
    let (h, p) = s.rsplit_once(':').ok_or_else(|| anyhow::anyhow!("expected host:port, got {s}"))?;
    Ok(Endpoint::base(h, p.parse()?))
}

fn station(
    name: &str,
    call: &Callsign,
    ep: Endpoint,
    work: &Path,
    bw: Bandwidth,
    radio: bool,
) -> anyhow::Result<Station> {
    let dir = work.join(format!("station-{name}"));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(Station::open(StationConfig {
        callsign: call.clone(),
        endpoint: ep,
        data_dir: dir,
        bandwidth: bw,
        max_file: 256 * 1024,
        chunk: 1024,
        auto_accept_files: true,
        receive_dir: None,
        tmp_dir: None,
        offer_timeout: Duration::from_secs(120),
        free_space: mc_core::station::FreeSpace::system(),
        hello_timeout: if radio { Duration::from_secs(240) } else { Duration::from_secs(5) },
        timeouts: Timeouts::default(),
        proto_minor: mch_protocol::MINOR,
        capabilities: mc_core::session::default_capabilities(),
        busy_detect: false, // the lab client keeps Mercury as configured
    })?)
}

struct FakeFaults(Arc<mercury_adapter::fake::FakeNet>);
impl Faults for FakeFaults {
    fn before_uncertain_send(&self) {
        self.0.set_hold(true);
    }
    fn after_drop(&self) {
        self.0.release_held();
    }
}

struct RadioFaults;
impl Faults for RadioFaults {}

fn write_outputs(work: &Path, report: &Report, a: &lab::Snapshot, b: &lab::Snapshot) -> anyhow::Result<()> {
    std::fs::write(work.join("report.json"), serde_json::to_string_pretty(report)?)?;
    let mut md = format!(
        "# M0 run — {}\n\n{} passed, {} failed\n\n| Step | Result | ms | Detail |\n|---|---|---|---|\n",
        report.mode, report.passed, report.failed
    );
    for s in &report.steps {
        md += &format!(
            "| {} | {} | {} | {} |\n",
            s.step,
            if s.pass { "PASS" } else { "**FAIL**" },
            s.millis,
            s.detail.replace('|', "\\|")
        );
    }
    if !report.measurements.is_empty() {
        md += "\n## Measurements\n\n";
        for (k, v) in &report.measurements {
            md += &format!("- {k}: {v}\n");
        }
    }
    std::fs::write(work.join("report.md"), md)?;
    for (n, s) in [("a", a), ("b", b)] {
        let t: String = s.transcript.iter().map(|l| format!("{:>9} g{} {}\n", l.t_ms, l.generation, l.line)).collect();
        std::fs::write(work.join(format!("transcript-{n}.txt")), t)?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Peer { host, port, call, echo, work } => {
            use sha2::{Digest, Sha256};
            std::fs::create_dir_all(&work)?;
            let c = Callsign::parse(&call)?;
            let mut cfg = StationConfig::defaults(c.clone(), Endpoint::base(&host, port), &work);
            // A real Mercury takes tens of seconds for HELLO (M0: 23 s at BW2300).
            cfg.hello_timeout = mc_core::station::RADIO_HELLO_TIMEOUT;
            let mut st = Station::open(cfg)?;
            st.attach().await?;
            st.set_listen(true).await?;
            println!(
                "{}",
                serde_json::json!({ "event": "listening", "call": c.as_str(), "host": host, "port": port, "echo": echo })
            );
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(mc_core::sim::serve_peer(st, echo, Some(tx)));
            while let Some((from, id, text)) = rx.recv().await {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "received",
                        "utc": mc_storage::now_utc(),
                        "peer": from,
                        "id": id.to_hex(),
                        "bytes": text.len(),
                        "sha256": hex::encode(Sha256::digest(text.as_bytes())),
                    })
                );
            }
            anyhow::bail!("the peer station stopped: its Mercury control connection is gone");
        }
        Cmd::Selftest { work } => {
            std::fs::create_dir_all(&work)?;
            let net = mercury_adapter::fake::FakeNet::start().await;
            let (ca, cb) = (Callsign::parse("LABA")?, Callsign::parse("LABB")?);
            let mut a = Handle::new(
                "A",
                station("a", &ca, Endpoint::base("127.0.0.1", net.a_base), &work, Bandwidth::Bw2300, false)?,
                "LABB",
            );
            let mut b = Handle::new(
                "B",
                station("b", &cb, Endpoint::base("127.0.0.1", net.b_base), &work, Bandwidth::Bw2300, false)?,
                "LABA",
            );
            let faults = FakeFaults(net.clone());
            let report =
                scenario::run("L0 fake Mercury pair", &mut a, &mut b, &ca, &cb, &work, &faults, &Budgets::fast()).await;
            let (sa, sb) = (a.snapshot().await, b.snapshot().await);
            write_outputs(&work, &report, &sa, &sb)?;
            println!("{} passed, {} failed — {}", report.passed, report.failed, work.join("report.md").display());
            std::process::exit(if report.failed == 0 { 0 } else { 1 });
        }
        Cmd::Lab { a, b, call_a, call_b, work, file_bytes, bw } => {
            std::fs::create_dir_all(&work)?;
            let bw = match bw.as_str() {
                "500" => Bandwidth::Bw500,
                "2750" => Bandwidth::Bw2750,
                _ => Bandwidth::Bw2300,
            };
            let (ca, cb) = (Callsign::parse(&call_a)?, Callsign::parse(&call_b)?);
            let mut ha = Handle::new("A", station("a", &ca, endpoint(&a)?, &work, bw, true)?, cb.as_str());
            let mut hb = Handle::new("B", station("b", &cb, endpoint(&b)?, &work, bw, true)?, ca.as_str());
            let mode = format!("L1 Mercury {a} <-> {b}, BW{}", bw.token());
            let report =
                scenario::run(&mode, &mut ha, &mut hb, &ca, &cb, &work, &RadioFaults, &Budgets::radio(file_bytes))
                    .await;
            let (sa, sb) = (ha.snapshot().await, hb.snapshot().await);
            write_outputs(&work, &report, &sa, &sb)?;
            println!("{} passed, {} failed — {}", report.passed, report.failed, work.join("report.md").display());
            std::process::exit(if report.failed == 0 { 0 } else { 1 });
        }
        Cmd::Broadcast { a, b, bcast_a, bcast_b, call_a, call_b, work } => {
            std::fs::create_dir_all(&work)?;
            let start = std::time::Instant::now();
            let mut sa = broadcast::Side::attach("A", Callsign::parse(&call_a)?, &endpoint(&a)?, bcast_a).await?;
            let mut sb = broadcast::Side::attach("B", Callsign::parse(&call_b)?, &endpoint(&b)?, bcast_b).await?;
            let report = broadcast::run(&mut sa, &mut sb).await;
            std::fs::write(work.join("report.json"), serde_json::to_string_pretty(&report)?)?;
            let mut md = format!(
                "# {}\n\n{} passed, {} failed\n\n| Step | Result | ms | Detail |\n|---|---|---|---|\n",
                report.mode, report.passed, report.failed
            );
            for st in &report.steps {
                md += &format!(
                    "| {} | {} | {} | {} |\n",
                    st.step,
                    if st.pass { "PASS" } else { "**FAIL**" },
                    st.millis,
                    st.detail.replace('|', "\\|")
                );
            }
            md += "\n## Measurements\n\n";
            for (k, v) in &report.measurements {
                md += &format!("- {k}: {v}\n");
            }
            std::fs::write(work.join("report.md"), md)?;
            std::fs::write(work.join("transcript-a.txt"), sa.transcript(start))?;
            std::fs::write(work.join("transcript-b.txt"), sb.transcript(start))?;
            println!("{} passed, {} failed — {}", report.passed, report.failed, work.join("report.md").display());
            std::process::exit(if report.failed == 0 { 0 } else { 1 });
        }
        Cmd::Probe { endpoint: ep, call, seconds } => {
            let ep = endpoint(&ep)?;
            let mut att = mercury_adapter::MercuryClient::attach(&ep, Timeouts::default()).await?;
            let c = Callsign::parse(&call)?;
            for cmd in [mercury_adapter::Command::MyCall(c), mercury_adapter::Command::Listen(false)] {
                println!("> {} => {:?}", cmd.wire().trim_end(), att.client.command(&cmd).await);
            }
            att.client.query("VERSION").await.ok();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);
            while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, att.events.recv()).await {
                if let mercury_adapter::AdapterEvent::RawLine(l) = ev {
                    println!("< {l}");
                }
            }
        }
    }
    Ok(())
}
