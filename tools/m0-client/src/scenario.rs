//! The M0 scenario: connect, text and binary exchange, receipts, duplicate
//! prevention, file integrity, disconnect/abort and recovery from interrupted
//! connections. Each step records PASS/FAIL with timings and evidence.

use crate::lab::Handle;
use mc_core::*;
use mch_protocol::ObjectId;
use mercury_adapter::{Callsign, Command};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Hooks that differ between the fake pair and real Mercury.
pub trait Faults: Send + Sync {
    /// Called right before the message whose delivery must become uncertain.
    fn before_uncertain_send(&self) {}
    /// Called after the sender's sockets were dropped.
    fn after_drop(&self) {}
}

#[derive(Clone, Debug)]
pub struct Budgets {
    pub call: Duration,
    pub receipt: Duration,
    pub file: Duration,
    pub disconnect: Duration,
    pub file_bytes: usize,
    pub chat_during_file_bytes: usize,
}

impl Budgets {
    pub fn fast() -> Self {
        Budgets {
            call: Duration::from_secs(5),
            receipt: Duration::from_secs(5),
            file: Duration::from_secs(10),
            disconnect: Duration::from_secs(5),
            file_bytes: 64 * 1024,
            chat_during_file_bytes: 32 * 1024,
        }
    }
    /// Real modem over audio: slow, generous budgets.
    pub fn radio(file_bytes: usize) -> Self {
        Budgets {
            call: Duration::from_secs(180),
            receipt: Duration::from_secs(300),
            file: Duration::from_secs(1800),
            disconnect: Duration::from_secs(600),
            file_bytes,
            chat_during_file_bytes: file_bytes,
        }
    }
}

#[derive(Serialize, Debug, Clone)]
pub struct StepResult {
    pub step: String,
    pub pass: bool,
    pub millis: u128,
    pub detail: String,
}

#[derive(Serialize, Debug)]
pub struct Report {
    pub mode: String,
    pub steps: Vec<StepResult>,
    pub measurements: Vec<(String, String)>,
    pub passed: usize,
    pub failed: usize,
}

pub struct Run {
    pub steps: Vec<StepResult>,
    pub measurements: Vec<(String, String)>,
}

impl Run {
    async fn step<F, Fut>(&mut self, name: &str, f: F) -> bool
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<String, String>>,
    {
        let t = Instant::now();
        eprintln!("--> {name}");
        let r = f().await;
        let (pass, detail) = match r {
            Ok(d) => (true, d),
            Err(d) => (false, d),
        };
        eprintln!("    {} ({} ms) {detail}", if pass { "PASS" } else { "FAIL" }, t.elapsed().as_millis());
        self.steps.push(StepResult { step: name.into(), pass, millis: t.elapsed().as_millis(), detail });
        pass
    }
    fn measure(&mut self, k: &str, v: impl Into<String>) {
        let v = v.into();
        eprintln!("    measure {k} = {v}");
        self.measurements.push((k.into(), v));
    }
}

fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn write_vector(dir: &Path, name: &str, len: usize, seed: u64) -> (PathBuf, Vec<u8>) {
    // Deterministic bytes covering all 256 values, KISS/MCH-looking sequences.
    let mut v = Vec::with_capacity(len);
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    for i in 0..len {
        if i < 256 {
            v.push(i as u8);
        } else if i % 97 == 0 {
            v.extend_from_slice(b"MCH1");
        } else {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            v.push((x >> 33) as u8);
        }
    }
    v.truncate(len);
    let p = dir.join(name);
    std::fs::write(&p, &v).unwrap();
    (p, v)
}

async fn connect_pair(
    a: &mut Handle,
    b: &mut Handle,
    call_b: &Callsign,
    t: Duration,
) -> Result<(ConnectInfo, ConnectInfo), String> {
    b.listen(true).await?;
    let (ia, ib) = tokio::join!(a.call(call_b, t), b.wait_incoming(t));
    let (ia, ib) = (ia?, ib?);
    // Receive-only again on B once the session is up: no unattended answers.
    a.start_session(ia.clone()).await;
    b.start_session(ib.clone()).await;
    Ok((ia, ib))
}

async fn negotiate(a: &mut Handle, b: &mut Handle, t: Duration) -> Result<String, String> {
    let ea = a.expect("Negotiated", t, |e| matches!(e, SessionEvent::Negotiated { .. })).await?;
    let eb = b.expect("Negotiated", t, |e| matches!(e, SessionEvent::Negotiated { .. })).await?;
    Ok(format!("A sees {ea:?}; B sees {eb:?}"))
}

async fn send_chat_and_confirm(
    from: &mut Handle,
    to: &mut Handle,
    text: &str,
    t: Duration,
) -> Result<(ObjectId, Duration), String> {
    let id = ObjectId::random();
    let start = Instant::now();
    from.send(SessionCommand::SendChat { id, text: text.into() }).await;
    let got =
        to.expect("ChatReceived", t, |e| matches!(e, SessionEvent::ChatReceived { id: i, .. } if *i == id)).await?;
    let SessionEvent::ChatReceived { text: body, duplicate, .. } = got else { unreachable!() };
    if body != text {
        return Err(format!("body mismatch: sent {} bytes, got {} bytes", text.len(), body.len()));
    }
    if duplicate {
        return Err("first delivery flagged duplicate".into());
    }
    from.expect("ChatDelivered", t, |e| matches!(e, SessionEvent::ChatDelivered { id: i, .. } if *i == id)).await?;
    Ok((id, start.elapsed()))
}

/// Prepare a lab vector as the app would (no photo shrink); `corrupt` makes
/// the declared SHA-256 wrong (the integrity fault).
fn prep(path: &Path, corrupt: bool) -> Result<(ObjectId, mc_core::files::Prepared), String> {
    let tmp = path.parent().unwrap_or(Path::new(".")).join("prep");
    let mut p = mc_core::files::prepare(path, mc_core::files::PhotoSize::Original, &tmp, 1 << 20)
        .map_err(|e| format!("prepare: {e:?}"))?;
    if corrupt {
        p.sha256 = "00".repeat(32);
    }
    Ok((ObjectId::random(), p))
}

async fn file_transfer(
    from: &mut Handle,
    to: &mut Handle,
    path: &Path,
    bytes: &[u8],
    t: Duration,
) -> Result<String, String> {
    from.clear_backlog();
    to.clear_backlog();
    let start = Instant::now();
    let (id, prepared) = prep(path, false)?;
    from.send(SessionCommand::SendFile { id, prepared }).await;
    let r = to
        .expect("FileReceived", t, |e| matches!(e, SessionEvent::FileReceived { .. } | SessionEvent::FileFailed { .. }))
        .await?;
    let SessionEvent::FileReceived { path: out, sha256, .. } = r else { return Err(format!("receiver: {r:?}")) };
    let got = std::fs::read(&out).map_err(|e| e.to_string())?;
    if got != bytes || sha256 != sha256_hex(bytes) {
        return Err(format!("content mismatch: {} vs {} bytes", got.len(), bytes.len()));
    }
    from.expect("FileDelivered", t, |e| matches!(e, SessionEvent::FileDelivered { .. })).await?;
    let secs = start.elapsed().as_secs_f64();
    Ok(format!(
        "{} bytes byte-exact, sha256 {}, receipt received; {:.1} s = {:.0} bit/s app payload",
        bytes.len(),
        &sha256[..16],
        secs,
        bytes.len() as f64 * 8.0 / secs
    ))
}

pub async fn run(
    mode: &str,
    a: &mut Handle,
    b: &mut Handle,
    call_a: &Callsign,
    call_b: &Callsign,
    work: &Path,
    faults: &dyn Faults,
    budget: &Budgets,
) -> Report {
    let mut run = Run { steps: Vec::new(), measurements: Vec::new() };
    let bud = budget.clone();
    std::fs::create_dir_all(work).ok();

    // 1. Attach in safe receive-only state.
    if !run
        .step("attach both stations (MYCALL, PUBLIC OFF, LISTEN OFF, BW)", || async {
            a.attach().await?;
            b.attach().await?;
            Ok("both attached; REGISTERED received".into())
        })
        .await
    {
        return finish(mode, run);
    }

    // 2. Connect A -> B, verify endpoint order and peer derivation.
    let mut ok = run
        .step("connect A->B and verify CONNECTED endpoint order", || async {
            let t = Instant::now();
            let (ia, ib) = connect_pair(a, b, call_b, bud.call).await?;
            if !(ia.initiator && !ib.initiator && ia.peer == call_b.as_str() && ib.peer == call_a.as_str()) {
                return Err(format!("unexpected endpoints A={ia:?} B={ib:?}"));
            }
            Ok(format!("CONNECTED {} {} {} after {} ms", ia.source, ia.dest, ia.bandwidth, t.elapsed().as_millis()))
        })
        .await;
    if ok {
        ok = run.step("MCH/1 HELLO negotiation", || negotiate(a, b, bud.receipt)).await;
    }
    if !ok {
        return finish(mode, run);
    }

    // 3. Text exchange with application receipts.
    let samples: Vec<(&str, String)> = vec![
        ("ascii", "CQ de LABA test 1".into()),
        ("multiline", "line one\nline two\r\nline three\n".into()),
        ("unicode", "Grüße ✓ Ω 73 😀 — 你好".into()),
        ("max 4096 bytes", "Z".repeat(4096)),
    ];
    for (label, text) in samples {
        let name = format!("chat A->B {label} with application receipt");
        let mut lat = None;
        run.step(&name, || async {
            let (_, d) = send_chat_and_confirm(a, b, &text, bud.receipt).await?;
            lat = Some(d);
            Ok(format!("{} bytes, delivered (receipt) in {} ms", text.len(), d.as_millis()))
        })
        .await;
        if let Some(d) = lat {
            run.measure(&format!("chat_latency_ms[{label}]"), d.as_millis().to_string());
        }
    }
    run.step("chat B->A with application receipt", || async {
        let (_, d) = send_chat_and_confirm(b, a, "73 from LABB", bud.receipt).await?;
        Ok(format!("delivered in {} ms", d.as_millis()))
    })
    .await;

    // 4. Binary file A -> B.
    let (fpath, fbytes) = write_vector(work, "vector-a.bin", bud.file_bytes, 1);
    run.step("binary file A->B: offer, accept, chunks, SHA-256, receipt", || {
        file_transfer(a, b, &fpath, &fbytes, bud.file)
    })
    .await;

    // 5. Chat priority while a file is queued B -> A.
    let (gpath, gbytes) = write_vector(work, "vector-b.bin", bud.chat_during_file_bytes, 2);
    run.step("chat A->B while file B->A is in progress", || async {
        a.clear_backlog();
        b.clear_backlog();
        let fstart = Instant::now();
        let (id, prepared) = prep(&gpath, false)?;
        b.send(SessionCommand::SendFile { id, prepared }).await;
        b.expect("first FileProgress", bud.file, |e| matches!(e, SessionEvent::FileProgress { .. })).await?;
        let (_, chat_lat) = send_chat_and_confirm(a, b, "chat during transfer", bud.receipt).await?;
        let chat_done = fstart.elapsed();
        let r = a
            .expect("FileReceived", bud.file, |e| {
                matches!(e, SessionEvent::FileReceived { .. } | SessionEvent::FileFailed { .. })
            })
            .await?;
        let SessionEvent::FileReceived { path, .. } = r else { return Err(format!("{r:?}")) };
        if std::fs::read(&path).map_err(|e| e.to_string())? != gbytes {
            return Err("file content mismatch".into());
        }
        b.expect("FileDelivered", bud.file, |e| matches!(e, SessionEvent::FileDelivered { .. })).await?;
        let file_done = fstart.elapsed();
        Ok(format!(
            "chat delivered {} ms after transfer start (latency {} ms); file finished at {} ms",
            chat_done.as_millis(),
            chat_lat.as_millis(),
            file_done.as_millis()
        ))
    })
    .await;

    // 6. Integrity: declared hash wrong -> no receipt, nothing published.
    let (hpath, _) = write_vector(work, "bad-hash.bin", 3000, 3);
    run.step("file integrity: wrong declared SHA-256 is refused (no receipt)", || async {
        a.clear_backlog();
        b.clear_backlog();
        let before = std::fs::read_dir(b.files_dir()).map(|d| d.count()).unwrap_or(0);
        let (id, prepared) = prep(&hpath, true)?;
        a.send(SessionCommand::SendFile { id, prepared }).await;
        let r = b
            .expect("FileFailed", bud.file, |e| {
                matches!(e, SessionEvent::FileFailed { .. } | SessionEvent::FileReceived { .. })
            })
            .await?;
        if !matches!(&r, SessionEvent::FileFailed { reason, .. } if reason.starts_with("hash_mismatch")) {
            return Err(format!("receiver: {r:?}"));
        }
        let s = a
            .expect("sender sees failure", bud.receipt, |e| {
                matches!(e, SessionEvent::FileFailed { .. } | SessionEvent::FileDelivered { .. })
            })
            .await?;
        if matches!(s, SessionEvent::FileDelivered { .. }) {
            return Err("sender saw a receipt for a corrupt file".into());
        }
        let after = std::fs::read_dir(b.files_dir()).map(|d| d.count()).unwrap_or(0);
        if after != before {
            return Err("a file was published".into());
        }
        Ok(format!("receiver: {r:?}; sender: {s:?}"))
    })
    .await;

    // 7. Graceful disconnect.
    run.step("graceful DISCONNECT from A; both sessions end", || async {
        let t = Instant::now();
        a.session_command(Command::Disconnect).await.map_err(|e| e.to_string())?;
        let ea = a.wait_end(bud.disconnect).await?;
        let eb = b.wait_end(bud.disconnect).await?;
        Ok(format!("A {ea:?}, B {eb:?} after {} ms", t.elapsed().as_millis()))
    })
    .await;

    // 8. Interrupted connection: A crashes after sending; message is uncertain.
    let uncertain_id = ObjectId::random();
    let uncertain_text = "sent just before the host crashed";
    let ok = run
        .step("reconnect, then drop A's sockets right after a send", || async {
            let (_, _) = connect_pair(a, b, call_b, bud.call).await?;
            negotiate(a, b, bud.receipt).await?;
            faults.before_uncertain_send();
            a.send(SessionCommand::SendChat { id: uncertain_id, text: uncertain_text.into() }).await;
            a.expect(
                "ChatSubmitted",
                bud.receipt,
                |e| matches!(e, SessionEvent::ChatSubmitted { id, .. } if *id == uncertain_id),
            )
            .await?;
            a.drop_sockets().await;
            let ea = a.wait_end(bud.receipt).await?;
            faults.after_drop();
            let row = a.messages().into_iter().find(|m| m.wire_id == uncertain_id.0);
            match row {
                Some(m) if m.state == "uncertain" => {
                    Ok(format!("A session end {ea:?}; message state 'uncertain' (not failed, not delivered)"))
                }
                other => Err(format!("A message row {other:?}")),
            }
        })
        .await;

    if ok {
        run.step("recover A: backoff re-attach, ABORT quarantine, stale data discarded", || async {
            let t = Instant::now();
            a.recover(6).await?;
            let eb = b.wait_end(bud.disconnect).await?;
            let snap = a.snapshot().await;
            Ok(format!(
                "A re-attached and quarantined in {} ms; B session ended {eb:?}; stale bytes discarded by A: {}",
                t.elapsed().as_millis(),
                snap.stale_bytes_discarded
            ))
        })
        .await;

        run.step("deliberate retry with the same message ID is deduplicated", || async {
            let first_copy = b.has_seen(|e| matches!(e, SessionEvent::ChatReceived { id, duplicate: false, .. } if *id == uncertain_id));
            a.clear_backlog();
            b.clear_backlog();
            connect_pair(a, b, call_b, bud.call).await?;
            negotiate(a, b, bud.receipt).await?;
            a.send(SessionCommand::SendChat { id: uncertain_id, text: uncertain_text.into() }).await;
            let r = b.expect("ChatReceived(retry)", bud.receipt, |e| matches!(e, SessionEvent::ChatReceived { id, .. } if *id == uncertain_id)).await?;
            a.expect("ChatDelivered(retry)", bud.receipt, |e| matches!(e, SessionEvent::ChatDelivered { id, .. } if *id == uncertain_id)).await?;
            let SessionEvent::ChatReceived { duplicate, .. } = r else { unreachable!() };
            let rows = b.messages().into_iter().filter(|m| m.wire_id == uncertain_id.0).count();
            if first_copy != duplicate {
                return Err(format!("first copy seen by B: {first_copy}, retry flagged duplicate: {duplicate}"));
            }
            if rows != 1 {
                return Err(format!("B stored {rows} copies"));
            }
            Ok(format!(
                "B had first copy: {first_copy}; retry duplicate={duplicate}; B stores exactly 1 copy; A now 'delivered'"
            ))
        }).await;
    }

    // 9. Abort while data is queued: never reported delivered without receipt.
    run.step("ABORT with a message queued: no false 'delivered'", || async {
        let id = ObjectId::random();
        faults.before_uncertain_send();
        a.send(SessionCommand::SendChat { id, text: "queued then aborted ".repeat(20) }).await;
        a.expect("ChatSubmitted", bud.receipt, |e| matches!(e, SessionEvent::ChatSubmitted { id: i, .. } if *i == id))
            .await?;
        a.session_command(Command::Abort).await.map_err(|e| e.to_string())?;
        let ea = a.wait_end(bud.disconnect).await?;
        faults.after_drop();
        let eb = b.wait_end(bud.disconnect).await?;
        let st = a.messages().into_iter().find(|m| m.wire_id == id.0).map(|m| m.state);
        if st.as_deref() == Some("delivered") {
            return Err("aborted message marked delivered".into());
        }
        Ok(format!("A {ea:?} / B {eb:?}; message state {st:?}"))
    })
    .await;

    // 10. Final consistency check.
    run.step("final stores: no duplicates at B, no delivered-without-receipt at A", || async {
        let sb = b.snapshot().await;
        let mut ids: Vec<_> = sb.messages.iter().filter(|m| m.direction == "in").map(|m| m.wire_id).collect();
        let n = ids.len();
        ids.sort();
        ids.dedup();
        if ids.len() != n {
            return Err("duplicate incoming message rows at B".into());
        }
        let sa = a.snapshot().await;
        let states: Vec<_> = sa.messages.iter().filter(|m| m.direction == "out").map(|m| m.state.clone()).collect();
        Ok(format!(
            "B incoming rows {n} (unique); A outgoing states {states:?}; PTT ON events A={} B={}",
            sa.ptt_events, sb.ptt_events
        ))
    })
    .await;

    finish(mode, run)
}

fn finish(mode: &str, run: Run) -> Report {
    let passed = run.steps.iter().filter(|s| s.pass).count();
    let failed = run.steps.len() - passed;
    Report { mode: mode.into(), steps: run.steps, measurements: run.measurements, passed, failed }
}
