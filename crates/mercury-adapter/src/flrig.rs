//! FLRig as the rig link (M3b.1 design §5.1): frequency and mode, read and
//! set, and PTT read, over FLRig's XML-RPC (HTTP POST to `/RPC2`). It speaks
//! the rigctld client's [`RigClientCmd`]/[`RigMsg`], so the rig policy cannot
//! tell them apart. MercuryChat never keys through FLRig: Mercury keys the
//! radio (owner decision, 21 Nov 2026), so `rig.set_ptt` is never sent.

use crate::rigctl::{PollRate, PttReading, RigCaps, RigClientCmd, RigLink, RigMsg, RigReply, RigRequest};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;

/// One XML-RPC call's time limit (connect, send, answer).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(1);
const BACKOFF: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(2), Duration::from_secs(5)];
const MAX_REPLY: usize = 64 * 1024;

/// An XML-RPC parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum XmlParam {
    Double(f64),
    Str(String),
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

pub fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The `<methodCall>` body.
pub fn method_call(method: &str, params: &[XmlParam]) -> String {
    let mut p = String::new();
    for x in params {
        let v = match x {
            XmlParam::Double(d) => format!("<double>{d}</double>"),
            XmlParam::Str(s) => format!("<string>{}</string>", xml_escape(s)),
        };
        p.push_str(&format!("<param><value>{v}</value></param>"));
    }
    format!(
        "<?xml version=\"1.0\"?><methodCall><methodName>{}</methodName><params>{p}</params></methodCall>",
        xml_escape(method)
    )
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)? + open.len();
    let b = s[a..].find(close)? + a;
    Some(&s[a..b])
}

/// The text of a `<value>`: its typed child's content, or its own text.
fn value_text(v: &str) -> String {
    let v = v.trim();
    if let Some(rest) = v.strip_prefix('<') {
        if let Some(end) = rest.find('>') {
            let tag = &rest[..end];
            if tag.ends_with('/') {
                return String::new(); // <string/>
            }
            if let Some(inner) = between(v, &format!("<{tag}>"), &format!("</{tag}>")) {
                return xml_unescape(inner.trim());
            }
        }
    }
    xml_unescape(v)
}

/// A `<methodResponse>`: the first parameter's text, or the fault's message.
pub fn parse_response(body: &str) -> Result<String, String> {
    if let Some(fault) = between(body, "<fault>", "</fault>") {
        let msg = fault
            .split("<member>")
            .find(|m| m.contains("faultString"))
            .and_then(|m| between(m, "<value>", "</value>"))
            .map(value_text)
            .unwrap_or_else(|| "FLRig refused the request".into());
        return Err(msg);
    }
    let param = between(body, "<param>", "</param>").ok_or("FLRig's answer has no value")?;
    Ok(value_text(between(param, "<value>", "</value>").ok_or("FLRig's answer has no value")?))
}

/// Why a call failed: FLRig answered with a fault, or it could not be reached.
#[derive(Debug, Clone, PartialEq)]
enum CallError {
    Fault(String),
    Transport(String),
}

async fn call(addr: &str, method: &str, params: &[XmlParam]) -> Result<String, CallError> {
    let body = method_call(method, params);
    let attempt = async {
        let mut s =
            tokio::net::TcpStream::connect(addr).await.map_err(|e| CallError::Transport(format!("{addr}: {e}")))?;
        let req = format!(
            "POST /RPC2 HTTP/1.1\r\nHost: {addr}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.map_err(|e| CallError::Transport(format!("write to FLRig: {e}")))?;
        let mut reply = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = s.read(&mut buf).await.map_err(|e| CallError::Transport(format!("read from FLRig: {e}")))?;
            if n == 0 {
                break;
            }
            reply.extend_from_slice(&buf[..n]);
            if reply.len() > MAX_REPLY {
                return Err(CallError::Transport("FLRig's answer is too long".into()));
            }
            if String::from_utf8_lossy(&reply).contains("</methodResponse>") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&reply).into_owned();
        if !text.starts_with("HTTP/") {
            return Err(CallError::Transport("not an HTTP answer: is this FLRig?".into()));
        }
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        parse_response(body).map_err(CallError::Fault)
    };
    tokio::time::timeout(CALL_TIMEOUT, attempt)
        .await
        .unwrap_or_else(|_| Err(CallError::Transport(format!("no answer from FLRig at {addr} within 1 s"))))
}

/// What FLRig can do, as the rig policy reads capabilities.
pub fn flrig_caps() -> RigCaps {
    RigCaps {
        model: "FLRig".into(),
        can_set_freq: true,
        can_set_mode: true,
        can_set_vfo: false,
        can_split: false,
        can_get_ptt: true,
        get_levels: vec![],
        set_levels: vec![],
        modes: vec![],
    }
}

/// A request as an FLRig call, or why it cannot be one.
fn as_call(req: &RigRequest) -> Result<(&'static str, Vec<XmlParam>), String> {
    match req {
        RigRequest::GetFreq => Ok(("rig.get_vfo", vec![])),
        RigRequest::SetFreq(hz) => Ok(("rig.set_frequency", vec![XmlParam::Double(*hz as f64)])),
        RigRequest::GetMode => Ok(("rig.get_mode", vec![])),
        RigRequest::SetMode { mode, .. } => Ok(("rig.set_mode", vec![XmlParam::Str(mode.clone())])),
        RigRequest::GetPtt => Ok(("rig.get_ptt", vec![])),
        _ => Err("not supported by FLRig".into()),
    }
}

fn reply_for(req: &RigRequest, text: &str) -> Result<RigReply, String> {
    match req {
        RigRequest::GetFreq => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && *f >= 0.0)
            .map(|f| RigReply::Freq(f.round() as u64))
            .ok_or_else(|| format!("FLRig gave {text:?} for the frequency")),
        RigRequest::GetMode => Ok(RigReply::Mode { mode: text.trim().to_string(), passband_hz: 0 }),
        RigRequest::GetPtt => match text.trim() {
            "0" => Ok(RigReply::Ptt(false)),
            "1" => Ok(RigReply::Ptt(true)),
            other => Err(format!("FLRig gave {other:?} for PTT")),
        },
        _ => Ok(RigReply::Ok),
    }
}

pub struct FlrigClient;

impl FlrigClient {
    /// Talk to FLRig at `addr` (`host:port`) until aborted. Frequency and
    /// mode are read every `read_every`; PTT at the [`PollRate`]'s pace.
    pub fn spawn(
        addr: String,
        read_every: Duration,
        cmds: mpsc::Receiver<RigClientCmd>,
        out: mpsc::Sender<RigMsg>,
    ) -> AbortHandle {
        tokio::spawn(run(addr, read_every, cmds, out)).abort_handle()
    }
}

async fn run(addr: String, read_every: Duration, mut cmds: mpsc::Receiver<RigClientCmd>, out: mpsc::Sender<RigMsg>) {
    let mut rate = PollRate::Idle;
    let mut step = 0usize;
    let mut lost_said = false;
    if out.send(RigMsg::Link(RigLink::Connecting)).await.is_err() {
        return;
    }
    loop {
        let reason = match serve(&addr, read_every, &mut cmds, &out, &mut rate, &mut step, &mut lost_said).await {
            None => return,
            Some(r) => r,
        };
        if !lost_said {
            lost_said = true;
            if out.send(RigMsg::Link(RigLink::Lost { reason })).await.is_err() {
                return;
            }
        }
        let until = Instant::now() + BACKOFF[step];
        step = (step + 1).min(BACKOFF.len() - 1);
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(until) => break,
                c = cmds.recv() => match c {
                    None => return,
                    Some(RigClientCmd::Rate(r)) => rate = r,
                    Some(RigClientCmd::Request { id, .. }) => {
                        if out.send(RigMsg::Done { id, result: Err("rig link down".into()) }).await.is_err() {
                            return;
                        }
                    }
                },
            }
        }
    }
}

/// One stretch of FLRig answering. `None`: stop; `Some(reason)`: lost.
async fn serve(
    addr: &str,
    read_every: Duration,
    cmds: &mut mpsc::Receiver<RigClientCmd>,
    out: &mpsc::Sender<RigMsg>,
    rate: &mut PollRate,
    step: &mut usize,
    lost_said: &mut bool,
) -> Option<String> {
    // Up once FLRig answers a frequency read.
    let first = match call(addr, "rig.get_vfo", &[]).await {
        Err(CallError::Transport(e)) => return Some(e),
        Err(CallError::Fault(e)) => return Some(format!("FLRig refused rig.get_vfo: {e}")),
        Ok(t) => t,
    };
    out.send(RigMsg::Caps(flrig_caps())).await.ok()?;
    out.send(RigMsg::Link(RigLink::Up)).await.ok()?;
    *step = 0;
    *lost_said = false;
    if let Ok(RigReply::Freq(hz)) = reply_for(&RigRequest::GetFreq, &first) {
        out.send(RigMsg::Freq(hz)).await.ok()?;
    }
    let now = Instant::now();
    let (mut next_ptt, mut next_read) = (now, now);
    loop {
        // The app's requests first.
        let next = match cmds.try_recv() {
            Ok(c) => Some(c),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => return None,
        };
        match next {
            Some(RigClientCmd::Rate(r)) => {
                if r != *rate && r == PollRate::Active {
                    next_ptt = Instant::now();
                }
                *rate = r;
                continue;
            }
            Some(RigClientCmd::Request { id, req }) => {
                if let Some(lost) = handle_now(addr, id, req, out, &mut next_read).await? {
                    return Some(lost);
                }
                continue;
            }
            None => {}
        }
        let now = Instant::now();
        let poll = if now >= next_ptt {
            next_ptt = now + rate.ptt_every();
            vec![RigRequest::GetPtt]
        } else if now >= next_read {
            next_read = now + read_every;
            vec![RigRequest::GetFreq, RigRequest::GetMode]
        } else {
            vec![]
        };
        if poll.is_empty() {
            tokio::select! {
                _ = tokio::time::sleep_until(next_ptt.min(next_read)) => {}
                c = cmds.recv() => match c {
                    None => return None,
                    Some(RigClientCmd::Rate(r)) => {
                        if r != *rate && r == PollRate::Active {
                            next_ptt = Instant::now();
                        }
                        *rate = r;
                    }
                    Some(RigClientCmd::Request { id, req }) => {
                        if let Some(lost) = handle_now(addr, id, req, out, &mut next_read).await? {
                            return Some(lost);
                        }
                    }
                },
            }
            continue;
        }
        for req in poll {
            let (method, params) = as_call(&req).expect("polls are FLRig calls");
            let msg = match call(addr, method, &params).await {
                Err(CallError::Transport(e)) => return Some(e),
                Err(CallError::Fault(e)) if req == RigRequest::GetPtt => RigMsg::Ptt(PttReading::Unknown { reason: e }),
                Err(CallError::Fault(_)) => continue, // a failed background read: the reading ages
                Ok(t) => match (&req, reply_for(&req, &t)) {
                    (_, Ok(RigReply::Freq(hz))) => RigMsg::Freq(hz),
                    (_, Ok(RigReply::Mode { mode, passband_hz })) => RigMsg::Mode { mode, passband_hz },
                    (_, Ok(RigReply::Ptt(on))) => RigMsg::Ptt(if on { PttReading::On } else { PttReading::Off }),
                    (RigRequest::GetPtt, Err(e)) => RigMsg::Ptt(PttReading::Unknown { reason: e }),
                    _ => continue,
                },
            };
            out.send(msg).await.ok()?;
        }
    }
}

/// Carry out one of the app's requests. `None`: stop; `Some(Some(reason))`:
/// lost. A successful set brings the next frequency and mode read forward.
async fn handle_now(
    addr: &str,
    id: u64,
    req: RigRequest,
    out: &mpsc::Sender<RigMsg>,
    next_read: &mut Instant,
) -> Option<Option<String>> {
    let result = match as_call(&req) {
        Err(e) => Err(e),
        Ok((method, params)) => match call(addr, method, &params).await {
            Err(CallError::Transport(e)) => {
                out.send(RigMsg::Done { id, result: Err(e.clone()) }).await.ok()?;
                return Some(Some(e));
            }
            Err(CallError::Fault(e)) => Err(e),
            Ok(t) => reply_for(&req, &t),
        },
    };
    if result.is_ok() && matches!(req, RigRequest::SetFreq(_) | RigRequest::SetMode { .. }) {
        *next_read = Instant::now();
    }
    out.send(RigMsg::Done { id, result }).await.ok()?;
    Some(None)
}
