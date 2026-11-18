//! The Settings link test (M3b): one short-lived connection to a rigctld,
//! separate from the running client, that only reads (`\dump_caps`, `f`,
//! `m`, `t`) and says, step by step, what worked.

use super::proto::{block_complete, parse_reply, RigProtoError, RigReply, RigRequest};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// One step's outcome: `ok` is `None` when the step was skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeStep {
    pub name: &'static str,
    pub ok: Option<bool>,
    pub detail: String,
}

/// What `t` answers when rigctld has no way to read PTT.
pub const RPRT_PTT_UNREADABLE: i32 = -11;

fn step(name: &'static str, ok: Option<bool>, detail: impl Into<String>) -> ProbeStep {
    ProbeStep { name, ok, detail: detail.into() }
}

async fn ask(sock: &mut TcpStream, req: &RigRequest, timeout: Duration) -> Result<RigReply, String> {
    let io = async {
        sock.write_all(req.wire().as_bytes()).await.map_err(|e| format!("could not send: {e}"))?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = sock.read(&mut chunk).await.map_err(|e| format!("could not read: {e}"))?;
            if n == 0 {
                return Err("rigctld closed the connection".to_string());
            }
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if block_complete(&text) {
                return parse_reply(req, &text).map_err(|e| match e {
                    RigProtoError::Rprt(n) => format!("RPRT {n}"),
                    other => format!("{other:?}"),
                });
            }
        }
    };
    tokio::time::timeout(timeout, io).await.unwrap_or_else(|_| Err(format!("no answer within {} s", timeout.as_secs())))
}

/// Connect to `addr` and read the rig's capabilities, frequency, mode and
/// PTT, each within `timeout`. A step after a failed connection, or after a
/// read that left the connection unusable, is skipped.
pub async fn probe(addr: &str, timeout: Duration) -> Vec<ProbeStep> {
    let mut steps = Vec::new();
    let names = ["caps", "frequency", "mode", "ptt"];
    let mut sock = match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => {
            steps.push(step("connect", Some(true), format!("connected to {addr}")));
            s
        }
        Ok(Err(e)) => {
            steps.push(step("connect", Some(false), format!("could not connect to {addr}: {e}")));
            steps.extend(names.iter().map(|n| step(n, None, "skipped: not connected")));
            return steps;
        }
        Err(_) => {
            steps.push(step("connect", Some(false), format!("no answer from {addr} within {} s", timeout.as_secs())));
            steps.extend(names.iter().map(|n| step(n, None, "skipped: not connected")));
            return steps;
        }
    };
    let reqs = [RigRequest::DumpCaps, RigRequest::GetFreq, RigRequest::GetMode, RigRequest::GetPtt];
    let mut usable = true;
    for (name, req) in names.into_iter().zip(reqs) {
        if !usable {
            steps.push(step(name, None, "skipped: the connection stopped answering"));
            continue;
        }
        let s = match ask(&mut sock, &req, timeout).await {
            Ok(RigReply::Caps(c)) => step(name, Some(true), format!("{} ({} modes)", c.model, c.modes.len())),
            Ok(RigReply::Freq(hz)) => step(name, Some(true), format!("{:.6} MHz", hz as f64 / 1e6)),
            Ok(RigReply::Mode { mode, passband_hz }) => step(name, Some(true), format!("{mode}, {passband_hz} Hz")),
            Ok(RigReply::Ptt(on)) => step(name, Some(true), if on { "transmitting" } else { "receiving" }),
            Ok(other) => step(name, Some(false), format!("unexpected answer {other:?}")),
            Err(e) if name == "ptt" && e == format!("RPRT {RPRT_PTT_UNREADABLE}") => step(
                name,
                Some(false),
                "rigctld cannot read PTT: start it with -P RIG (or your rig's PTT type), or MercuryChat cannot check transmissions",
            ),
            Err(e) => {
                usable = e.starts_with("RPRT");
                step(name, Some(false), e)
            }
        };
        steps.push(s);
    }
    steps
}
