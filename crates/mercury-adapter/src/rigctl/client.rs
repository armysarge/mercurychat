//! The rigctld client task (M3b design §7.1): one TCP connection to an
//! operator-run rigctld, one request in flight, polling, timeouts and
//! reconnect. It carries out requests and reports what the rig says; every
//! rule about *when* a change is allowed lives in the app (mc-core's rig
//! policy), never here.

use super::proto::{block_complete, parse_reply, Level, RigCaps, RigProtoError, RigReply, RigRequest};
use crate::telemetry::{jitter, BACKOFF};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;

/// How often to read the rig's PTT: fast during activity (a session, a CQ or
/// beacon, Tune, or Mercury reporting PTT on), slow otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollRate {
    Active,
    Idle,
}

impl PollRate {
    fn ptt_every(self) -> Duration {
        match self {
            PollRate::Active => Duration::from_millis(250),
            PollRate::Idle => Duration::from_secs(1),
        }
    }

    fn ptt_timeout(self) -> Duration {
        match self {
            PollRate::Active => Duration::from_millis(200),
            PollRate::Idle => Duration::from_millis(500),
        }
    }
}

/// Frequency and mode are read this often at either rate.
pub const READ_EVERY: Duration = Duration::from_secs(1);
/// Meters are read this often while the rig reports PTT on.
pub const METER_EVERY: Duration = Duration::from_millis(250);
/// Timeout for anything but `t`.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
/// `\dump_caps` is long; it gets longer.
pub const CAPS_TIMEOUT: Duration = Duration::from_secs(3);
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(3);
/// This many timeouts in a row and rigctld is taken to have stopped answering.
pub const MAX_TIMEOUTS: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub enum RigClientCmd {
    /// Carry out `req`; the answer comes back as [`RigMsg::Done`] with `id`.
    Request {
        id: u64,
        req: RigRequest,
    },
    Rate(PollRate),
}

/// The rig's PTT as read. Anything but a clean `PTT: 0` or `PTT: 1` reply
/// (a timeout, an `RPRT` error, garbage) is unknown, never off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PttReading {
    On,
    Off,
    Unknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigLink {
    Connecting,
    Up,
    /// Said once per loss, not per failed retry.
    Lost {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum RigMsg {
    Link(RigLink),
    Caps(RigCaps),
    Freq(u64),
    Mode { mode: String, passband_hz: i32 },
    Vfo(String),
    Split { on: bool, tx_vfo: String },
    Ptt(PttReading),
    Meter(Level, f32),
    Done { id: u64, result: Result<RigReply, String> },
}

/// A connection to rigctld.
pub trait RigIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> RigIo for T {}
pub type RigConn = Pin<Box<dyn RigIo>>;

pub struct RigClient;

impl RigClient {
    /// Connect to rigctld at `addr` (`host:port`) and keep at it until aborted.
    pub fn spawn(addr: String, cmds: mpsc::Receiver<RigClientCmd>, out: mpsc::Sender<RigMsg>) -> AbortHandle {
        Self::spawn_with(addr, cmds, out, jitter)
    }

    /// As [`RigClient::spawn`], with the backoff jitter supplied (tests
    /// shorten it).
    pub fn spawn_with(
        addr: String,
        cmds: mpsc::Receiver<RigClientCmd>,
        out: mpsc::Sender<RigMsg>,
        jitter: impl FnMut(Duration) -> Duration + Send + 'static,
    ) -> AbortHandle {
        let dial = move || {
            let addr = addr.clone();
            async move {
                match tokio::time::timeout(DIAL_TIMEOUT, tokio::net::TcpStream::connect(&addr)).await {
                    Err(_) => Err(format!("no answer from {addr} within {} s", DIAL_TIMEOUT.as_secs())),
                    Ok(Err(e)) => Err(format!("{addr}: {e}")),
                    Ok(Ok(s)) => {
                        let _ = s.set_nodelay(true);
                        Ok(Box::pin(s) as RigConn)
                    }
                }
            }
        };
        tokio::spawn(run(dial, cmds, out, jitter)).abort_handle()
    }

    /// As [`RigClient::spawn`] over any connection (tests, the simulator).
    pub fn spawn_dial<D, F>(
        dial: D,
        cmds: mpsc::Receiver<RigClientCmd>,
        out: mpsc::Sender<RigMsg>,
        jitter: impl FnMut(Duration) -> Duration + Send + 'static,
    ) -> AbortHandle
    where
        D: FnMut() -> F + Send + 'static,
        F: Future<Output = Result<RigConn, String>> + Send + 'static,
    {
        tokio::spawn(run(dial, cmds, out, jitter)).abort_handle()
    }
}

/// Why the session with rigctld ended.
enum End {
    /// The app dropped its command sender or its receiver: stop.
    Stop,
    Lost(String),
}

async fn run<D, F>(
    mut dial: D,
    mut cmds: mpsc::Receiver<RigClientCmd>,
    out: mpsc::Sender<RigMsg>,
    mut jitter: impl FnMut(Duration) -> Duration,
) where
    D: FnMut() -> F,
    F: Future<Output = Result<RigConn, String>>,
{
    let mut rate = PollRate::Idle;
    let mut step = 0usize;
    let mut lost_said = false;
    if out.send(RigMsg::Link(RigLink::Connecting)).await.is_err() {
        return;
    }
    loop {
        let reason = match dial().await {
            Err(e) => e,
            Ok(conn) => {
                let mut s = Session::new(conn, &out);
                match s.serve(&mut cmds, &mut rate, &mut step, &mut lost_said).await {
                    End::Stop => return,
                    End::Lost(r) => r,
                }
            }
        };
        if !lost_said {
            lost_said = true;
            if out.send(RigMsg::Link(RigLink::Lost { reason })).await.is_err() {
                return;
            }
        }
        // Wait out the backoff, failing requests at once meanwhile.
        let until = Instant::now() + jitter(BACKOFF[step]);
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

/// A queued request: the app's (with its id) or a poll.
struct Queued {
    id: Option<u64>,
    req: RigRequest,
}

struct Session<'a> {
    conn: RigConn,
    buf: String,
    out: &'a mpsc::Sender<RigMsg>,
    caps: RigCaps,
    keyed: bool,
    timeouts: u32,
    /// Replies still owed for requests that timed out. rigctld answers in
    /// order, so the next this-many blocks are theirs: stale, and dropped.
    late: usize,
}

/// The first line rigctld writes for a request's reply.
fn header(req: &RigRequest) -> &'static str {
    match req {
        RigRequest::GetFreq => "get_freq:",
        RigRequest::SetFreq(_) => "set_freq:",
        RigRequest::GetMode => "get_mode:",
        RigRequest::SetMode { .. } => "set_mode:",
        RigRequest::GetVfo => "get_vfo:",
        RigRequest::SetVfo(_) => "set_vfo:",
        RigRequest::GetSplit => "get_split_vfo:",
        RigRequest::SetSplit { .. } => "set_split_vfo:",
        RigRequest::GetPtt => "get_ptt:",
        RigRequest::PttOff => "set_ptt:",
        RigRequest::GetLevel(_) => "get_level:",
        RigRequest::SetLevel(..) => "set_level:",
        RigRequest::DumpCaps => "dump_caps:",
    }
}

impl<'a> Session<'a> {
    fn new(conn: RigConn, out: &'a mpsc::Sender<RigMsg>) -> Self {
        Session { conn, buf: String::new(), out, caps: RigCaps::default(), keyed: false, timeouts: 0, late: 0 }
    }

    async fn send(&mut self, m: RigMsg) -> Result<(), End> {
        self.out.send(m).await.map_err(|_| End::Stop)
    }

    async fn serve(
        &mut self,
        cmds: &mut mpsc::Receiver<RigClientCmd>,
        rate: &mut PollRate,
        step: &mut usize,
        lost_said: &mut bool,
    ) -> End {
        match self.request(&RigRequest::DumpCaps, CAPS_TIMEOUT).await {
            Ok(Ok(RigReply::Caps(c))) => self.caps = c,
            Ok(Ok(_)) => return End::Lost("rigctld gave no capabilities".into()),
            Ok(Err(e)) => return End::Lost(format!("rigctld refused \\dump_caps: {e}")),
            Err(end) => return end,
        }
        if let Err(e) = self.send(RigMsg::Caps(self.caps.clone())).await {
            return e;
        }
        if let Err(e) = self.send(RigMsg::Link(RigLink::Up)).await {
            return e;
        }
        *step = 0;
        *lost_said = false;

        let mut queue: VecDeque<Queued> = VecDeque::new();
        let now = Instant::now();
        let (mut next_ptt, mut next_read, mut next_meter) = (now, now, now);
        loop {
            // Take whatever the app has sent, without waiting.
            loop {
                match cmds.try_recv() {
                    Ok(RigClientCmd::Rate(r)) => {
                        if r != *rate && r == PollRate::Active {
                            next_ptt = Instant::now();
                        }
                        *rate = r;
                    }
                    Ok(RigClientCmd::Request { id, req }) => queue.push_back(Queued { id: Some(id), req }),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => return End::Stop,
                }
            }
            let now = Instant::now();
            if now >= next_ptt {
                next_ptt = now + rate.ptt_every();
                queue.push_back(Queued { id: None, req: RigRequest::GetPtt });
            }
            if now >= next_read {
                next_read = now + READ_EVERY;
                queue.push_back(Queued { id: None, req: RigRequest::GetFreq });
                queue.push_back(Queued { id: None, req: RigRequest::GetMode });
            }
            if self.keyed && now >= next_meter {
                next_meter = now + METER_EVERY;
                for l in [Level::Swr, Level::Alc, Level::RfPowerMeter] {
                    if self.caps.get_levels.contains(&l) {
                        queue.push_back(Queued { id: None, req: RigRequest::GetLevel(l) });
                    }
                }
            }
            if let Some(q) = queue.pop_front() {
                if !self.keyed && matches!(q.req, RigRequest::GetLevel(l) if l != Level::RfPower) && q.id.is_none() {
                    continue; // unkeyed since it was queued: meters mean nothing now
                }
                let limit = if q.req == RigRequest::GetPtt { rate.ptt_timeout() } else { REQUEST_TIMEOUT };
                let result = match self.request(&q.req, limit).await {
                    Ok(r) => r,
                    Err(end) => return end,
                };
                if let Err(e) = self.report(q, result, &mut queue).await {
                    return e;
                }
                if self.timeouts >= MAX_TIMEOUTS {
                    return End::Lost(format!("rigctld stopped answering ({MAX_TIMEOUTS} timeouts in a row)"));
                }
                continue;
            }
            // Nothing to do: sleep until the next poll or a command.
            let wake = if self.keyed { next_ptt.min(next_read).min(next_meter) } else { next_ptt.min(next_read) };
            tokio::select! {
                _ = tokio::time::sleep_until(wake) => {}
                c = cmds.recv() => match c {
                    None => return End::Stop,
                    Some(RigClientCmd::Rate(r)) => {
                        if r != *rate && r == PollRate::Active {
                            next_ptt = Instant::now();
                        }
                        *rate = r;
                    }
                    Some(RigClientCmd::Request { id, req }) => queue.push_back(Queued { id: Some(id), req }),
                },
            }
        }
    }

    /// What a reply means for the app.
    async fn report(
        &mut self,
        q: Queued,
        result: Result<RigReply, String>,
        queue: &mut VecDeque<Queued>,
    ) -> Result<(), End> {
        if let Some(id) = q.id {
            if result.is_ok() {
                // Follow a successful set with a fresh reading.
                match &q.req {
                    RigRequest::SetFreq(_) => queue.push_front(Queued { id: None, req: RigRequest::GetFreq }),
                    RigRequest::SetMode { .. } => queue.push_front(Queued { id: None, req: RigRequest::GetMode }),
                    RigRequest::SetVfo(_) => queue.push_front(Queued { id: None, req: RigRequest::GetVfo }),
                    RigRequest::SetSplit { .. } => queue.push_front(Queued { id: None, req: RigRequest::GetSplit }),
                    RigRequest::PttOff => queue.push_front(Queued { id: None, req: RigRequest::GetPtt }),
                    _ => {}
                }
            }
            return self.send(RigMsg::Done { id, result }).await;
        }
        let msg = match (&q.req, result) {
            (RigRequest::GetPtt, Ok(RigReply::Ptt(on))) => {
                self.keyed = on;
                RigMsg::Ptt(if on { PttReading::On } else { PttReading::Off })
            }
            (RigRequest::GetPtt, Ok(other)) => {
                RigMsg::Ptt(PttReading::Unknown { reason: format!("unexpected reply {other:?}") })
            }
            (RigRequest::GetPtt, Err(reason)) => {
                // Unknown is not unkeyed: keep reading meters if we were keyed.
                RigMsg::Ptt(PttReading::Unknown { reason })
            }
            (_, Ok(RigReply::Freq(hz))) => RigMsg::Freq(hz),
            (_, Ok(RigReply::Mode { mode, passband_hz })) => RigMsg::Mode { mode, passband_hz },
            (_, Ok(RigReply::Vfo(v))) => RigMsg::Vfo(v),
            (_, Ok(RigReply::Split { on, tx_vfo })) => RigMsg::Split { on, tx_vfo },
            (RigRequest::GetLevel(l), Ok(RigReply::Level(v))) => RigMsg::Meter(*l, v),
            // A failed background read is simply not reported: the reading ages.
            _ => return Ok(()),
        };
        self.send(msg).await
    }

    /// Send one request and read its reply. `Ok(Err(..))` is a failed
    /// request (an `RPRT` error, garbage, a timeout); `Err(End)` ends the
    /// connection.
    async fn request(&mut self, req: &RigRequest, limit: Duration) -> Result<Result<RigReply, String>, End> {
        if let Err(e) = self.conn.write_all(req.wire().as_bytes()).await {
            return Err(End::Lost(format!("write to rigctld failed: {e}")));
        }
        let deadline = Instant::now() + limit;
        let want = header(req);
        loop {
            // A complete block in the buffer? Replies to requests that timed
            // out earlier arrive late: drop them, and anything not ours.
            while let Some(block) = self.take_block() {
                if self.late > 0 {
                    self.late -= 1;
                    continue;
                }
                if block.lines().next().is_some_and(|l| l.starts_with(want)) {
                    self.timeouts = 0;
                    return Ok(match parse_reply(req, &block) {
                        Ok(r) => Ok(r),
                        Err(RigProtoError::Rprt(n)) => Err(format!("RPRT {n}")),
                        Err(RigProtoError::Malformed(m)) => Err(format!("malformed reply: {m}")),
                        Err(RigProtoError::Incomplete) => Err("incomplete reply".into()),
                    });
                }
            }
            let mut chunk = [0u8; 4096];
            match tokio::time::timeout_at(deadline, self.conn.read(&mut chunk)).await {
                Err(_) => {
                    self.timeouts += 1;
                    self.late += 1;
                    return Ok(Err(format!("no reply within {} ms", limit.as_millis())));
                }
                Ok(Ok(0)) => return Err(End::Lost("rigctld closed the connection".into())),
                Ok(Ok(n)) => {
                    self.buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    if self.buf.len() > 1 << 20 {
                        return Err(End::Lost("rigctld sent more than 1 MiB without a reply end".into()));
                    }
                }
                Ok(Err(e)) => return Err(End::Lost(format!("read from rigctld failed: {e}"))),
            }
        }
    }

    /// Split the first complete reply block (through its `RPRT` line) off the buffer.
    fn take_block(&mut self) -> Option<String> {
        if !block_complete(&self.buf) {
            return None;
        }
        let mut end = 0;
        for line in self.buf.split_inclusive('\n') {
            end += line.len();
            if line.trim_end().starts_with("RPRT ") && line.ends_with('\n') {
                break;
            }
        }
        let block = self.buf[..end].to_string();
        self.buf.drain(..end);
        Some(block)
    }
}
