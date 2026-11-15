//! A station running in its own task, driven by the scenario through channels.

use mc_core::*;
use mercury_adapter::{Ack, Callsign, Command, CommandError};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

type Reply<T> = oneshot::Sender<T>;

pub enum Ctl {
    Attach(Reply<Result<(), String>>),
    Recover(u32, Reply<Result<(), String>>),
    Listen(bool, Reply<Result<(), String>>),
    Call(Callsign, Duration, Reply<Result<ConnectInfo, String>>),
    WaitIncoming(Duration, Reply<Result<ConnectInfo, String>>),
    Session {
        info: ConnectInfo,
        cmds: mpsc::Receiver<SessionCommand>,
        reqs: mpsc::Receiver<StationRequest>,
        end: Reply<SessionEnd>,
    },
    Snapshot(Reply<Snapshot>),
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub transcript: Vec<TranscriptLine>,
    pub ptt_events: u64,
    pub stale_bytes_discarded: u64,
    pub messages: Vec<mc_storage::MessageRow>,
}

pub fn spawn_station(st: Station, peer_hint: String) -> (mpsc::Sender<Ctl>, mpsc::UnboundedReceiver<SessionEvent>) {
    let (tx, mut rx) = mpsc::channel::<Ctl>(16);
    let (ev_tx, ev_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut st = st;
        while let Some(c) = rx.recv().await {
            match c {
                Ctl::Attach(r) => {
                    let _ = r.send(st.attach().await.map_err(|e| e.to_string()));
                }
                Ctl::Recover(n, r) => {
                    let _ = r.send(st.recover(n).await.map_err(|e| e.to_string()));
                }
                Ctl::Listen(on, r) => {
                    let _ = r.send(st.set_listen(on).await.map_err(|e| e.to_string()));
                }
                Ctl::Call(p, t, r) => {
                    let _ = r.send(st.call(&p, t).await.map_err(|e| e.to_string()));
                }
                Ctl::WaitIncoming(t, r) => {
                    let _ = r.send(st.wait_incoming(t).await.map_err(|e| e.to_string()));
                }
                Ctl::Session { info, mut cmds, mut reqs, end } => {
                    let e = st.run_session(&info, &mut cmds, &mut reqs, ev_tx.clone()).await;
                    let _ = end.send(e);
                }
                Ctl::Snapshot(r) => {
                    let messages = st.store.messages(&peer_hint).unwrap_or_default();
                    let _ = r.send(Snapshot {
                        transcript: st.transcript.clone(),
                        ptt_events: st.ptt_events,
                        stale_bytes_discarded: st.stale_bytes_discarded,
                        messages,
                    });
                }
            }
        }
    });
    (tx, ev_rx)
}

pub struct Handle {
    pub name: &'static str,
    pub ctl: mpsc::Sender<Ctl>,
    pub events: mpsc::UnboundedReceiver<SessionEvent>,
    /// Every event received, for the run log.
    pub seen: Vec<(Instant, SessionEvent)>,
    /// Events received but not yet consumed by an `expect`.
    backlog: std::collections::VecDeque<SessionEvent>,
    data_dir: std::path::PathBuf,
    peer_hint: String,
    sess: Option<(mpsc::Sender<SessionCommand>, mpsc::Sender<StationRequest>, oneshot::Receiver<SessionEnd>)>,
}

impl Handle {
    pub fn new(name: &'static str, st: Station, peer_hint: &str) -> Self {
        let data_dir = st.cfg.data_dir.clone();
        let (ctl, events) = spawn_station(st, peer_hint.to_string());
        Handle {
            name,
            ctl,
            events,
            seen: Vec::new(),
            backlog: Default::default(),
            data_dir,
            peer_hint: peer_hint.to_string(),
            sess: None,
        }
    }

    pub fn files_dir(&self) -> std::path::PathBuf {
        self.data_dir.join("files")
    }

    /// Read the station's messages through a second SQLite connection, so it
    /// works while a session holds the station task.
    pub fn messages(&self) -> Vec<mc_storage::MessageRow> {
        mc_storage::Store::open(&self.data_dir.join("station.db"))
            .and_then(|s| s.messages(&self.peer_hint))
            .unwrap_or_default()
    }

    /// True if an event matching `pred` was already received (consumed or not).
    pub fn has_seen<F: Fn(&SessionEvent) -> bool>(&mut self, pred: F) -> bool {
        self.drain();
        self.seen.iter().any(|(_, e)| pred(e))
    }

    async fn ask<T>(&self, f: impl FnOnce(Reply<T>) -> Ctl) -> T {
        let (tx, rx) = oneshot::channel();
        self.ctl.send(f(tx)).await.expect("station task alive");
        rx.await.expect("station replied")
    }

    pub async fn attach(&self) -> Result<(), String> {
        self.ask(Ctl::Attach).await
    }
    pub async fn recover(&self, n: u32) -> Result<(), String> {
        self.ask(|r| Ctl::Recover(n, r)).await
    }
    pub async fn listen(&self, on: bool) -> Result<(), String> {
        self.ask(|r| Ctl::Listen(on, r)).await
    }
    pub async fn call(&self, p: &Callsign, t: Duration) -> Result<ConnectInfo, String> {
        self.ask(|r| Ctl::Call(p.clone(), t, r)).await
    }
    pub async fn wait_incoming(&self, t: Duration) -> Result<ConnectInfo, String> {
        self.ask(|r| Ctl::WaitIncoming(t, r)).await
    }
    /// Station-side snapshot (transcript, counters). Only answered while no
    /// session is running; times out rather than hanging otherwise.
    pub async fn snapshot(&self) -> Snapshot {
        match tokio::time::timeout(Duration::from_secs(5), self.ask(Ctl::Snapshot)).await {
            Ok(s) => s,
            Err(_) => {
                Snapshot { transcript: Vec::new(), ptt_events: 0, stale_bytes_discarded: 0, messages: self.messages() }
            }
        }
    }

    pub async fn start_session(&mut self, info: ConnectInfo) {
        let (ctx, crx) = mpsc::channel(64);
        let (rtx, rrx) = mpsc::channel(8);
        let (etx, erx) = oneshot::channel();
        self.ctl.send(Ctl::Session { info, cmds: crx, reqs: rrx, end: etx }).await.unwrap();
        self.sess = Some((ctx, rtx, erx));
    }

    pub async fn send(&self, c: SessionCommand) {
        if let Some((tx, _, _)) = &self.sess {
            let _ = tx.send(c).await;
        }
    }

    /// Issue DISCONNECT/ABORT while the session runs.
    pub async fn session_command(&self, c: Command) -> Result<Ack, CommandError> {
        let (tx, rx) = oneshot::channel();
        let (_, reqs, _) = self.sess.as_ref().expect("session running");
        reqs.send(StationRequest::Command(c, tx)).await.map_err(|_| CommandError::Closed)?;
        rx.await.map_err(|_| CommandError::Closed)?
    }

    pub async fn drop_sockets(&self) {
        if let Some((_, reqs, _)) = &self.sess {
            let _ = reqs.send(StationRequest::DropSockets).await;
        }
    }

    pub async fn wait_end(&mut self, t: Duration) -> Result<SessionEnd, String> {
        let (_, _, end) = self.sess.as_mut().ok_or("no session")?;
        let r = tokio::time::timeout(t, end).await;
        self.sess = None;
        match r {
            Ok(Ok(e)) => Ok(e),
            Ok(Err(_)) => Err("station task dropped session".into()),
            Err(_) => Err(format!("{}: session did not end within {t:?}", self.name)),
        }
    }

    /// Wait for an event matching `pred`; all events are kept in `seen`.
    pub async fn expect<F>(&mut self, what: &str, t: Duration, mut pred: F) -> Result<SessionEvent, String>
    where
        F: FnMut(&SessionEvent) -> bool,
    {
        if let Some(i) = self.backlog.iter().position(&mut pred) {
            return Ok(self.backlog.remove(i).unwrap());
        }
        let deadline = tokio::time::Instant::now() + t;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Err(_) => return Err(format!("{}: timed out after {t:?} waiting for {what}", self.name)),
                Ok(None) => return Err(format!("{}: event channel closed", self.name)),
                Ok(Some(e)) => {
                    self.seen.push((Instant::now(), e.clone()));
                    if pred(&e) {
                        return Ok(e);
                    }
                    self.backlog.push_back(e);
                }
            }
        }
    }

    /// Non-blocking drain of pending events into `seen` and the backlog.
    pub fn drain(&mut self) {
        while let Ok(e) = self.events.try_recv() {
            self.seen.push((Instant::now(), e.clone()));
            self.backlog.push_back(e);
        }
    }

    /// Forget unconsumed events (between scenario steps).
    pub fn clear_backlog(&mut self) {
        self.drain();
        self.backlog.clear();
    }
}
