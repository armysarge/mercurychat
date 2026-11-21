//! A fake rigctld for tests and the simulator (feature `fake`): a loopback
//! TCP server that answers rigctld's extended protocol for one dummy rig
//! shared by every client, like `rigctld -m 1`. Tests script it: key it as
//! Mercury would, make `t` fail, slow every reply, change its capabilities,
//! kill it and bring it back on the same port. It logs every request line.

use crate::rigctl::{Level, RigCaps};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpSocket};
use tokio::task::JoinHandle;

struct State {
    freq: u64,
    mode: String,
    passband: i32,
    vfo: String,
    split: bool,
    tx_vfo: String,
    ptt: bool,
    ptt_error: Option<i32>,
    delay: Duration,
    caps: RigCaps,
    levels: HashMap<&'static str, f32>,
    log: Vec<String>,
    tasks: Vec<JoinHandle<()>>,
}

/// The capabilities `rigctld -m 1` reports (tests/fixtures/rigctld/dump_caps.txt).
pub fn dummy_caps() -> RigCaps {
    RigCaps {
        model: "Dummy".into(),
        can_set_freq: true,
        can_set_mode: true,
        can_set_vfo: true,
        can_split: true,
        can_get_ptt: true,
        get_levels: Level::ALL.to_vec(),
        set_levels: vec![Level::RfPower],
        modes: ["AM", "CW", "USB", "LSB", "RTTY", "FM", "PKTUSB", "PKTLSB"].map(String::from).to_vec(),
    }
}

pub struct FakeRigctld {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl FakeRigctld {
    pub async fn start() -> FakeRigctld {
        Self::start_at("127.0.0.1:0".parse().unwrap()).await
    }

    /// As [`FakeRigctld::start`], listening at `addr` (the fake-rigctld program).
    pub async fn start_at(addr: SocketAddr) -> FakeRigctld {
        let state = Arc::new(Mutex::new(State {
            freq: 14_074_000,
            mode: "PKTUSB".into(),
            passband: 3000,
            vfo: "VFOA".into(),
            split: false,
            tx_vfo: "None".into(),
            ptt: false,
            ptt_error: None,
            delay: Duration::ZERO,
            caps: dummy_caps(),
            levels: HashMap::from([("RFPOWER", 0.5), ("SWR", 1.2), ("ALC", 0.1), ("RFPOWER_METER", 0.4)]),
            log: vec![],
            tasks: vec![],
        }));
        let listener = TcpListener::bind(addr).await.expect("bind fake rigctld");
        let addr = listener.local_addr().unwrap();
        let fake = FakeRigctld { addr, state };
        fake.serve(listener);
        fake
    }

    fn serve(&self, listener: TcpListener) {
        let state = self.state.clone();
        let accept = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let st = state.clone();
                let conn = tokio::spawn(client(sock, st));
                state.lock().unwrap().tasks.push(conn);
            }
        });
        self.state.lock().unwrap().tasks.push(accept);
    }

    /// The rig keyed (or unkeyed) by someone else: Mercury, in the real setup.
    pub fn set_ptt(&self, on: bool) {
        self.state.lock().unwrap().ptt = on;
    }
    pub fn ptt(&self) -> bool {
        self.state.lock().unwrap().ptt
    }
    /// Answer `t` with this `RPRT` error (`None`: answer normally).
    pub fn ptt_error(&self, code: Option<i32>) {
        self.state.lock().unwrap().ptt_error = code;
    }
    /// Wait this long before every reply.
    pub fn delay(&self, d: Duration) {
        self.state.lock().unwrap().delay = d;
    }
    pub fn caps(&self, caps: RigCaps) {
        self.state.lock().unwrap().caps = caps;
    }
    pub fn freq(&self) -> u64 {
        self.state.lock().unwrap().freq
    }
    pub fn set_freq(&self, hz: u64) {
        self.state.lock().unwrap().freq = hz;
    }
    pub fn mode(&self) -> (String, i32) {
        let s = self.state.lock().unwrap();
        (s.mode.clone(), s.passband)
    }
    /// Every request line received, oldest first, without the newline.
    pub fn log(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }
    /// Stop listening and drop every client, as a killed rigctld would.
    pub fn kill(&self) {
        for t in self.state.lock().unwrap().tasks.drain(..) {
            t.abort();
        }
    }
    /// Listen again on the same port. The rig keeps its state, as a real
    /// rig does when only rigctld restarts.
    pub async fn restart(&self) {
        let sock = TcpSocket::new_v4().unwrap();
        let _ = sock.set_reuseaddr(true);
        let mut tries = 0;
        let listener = loop {
            match sock.bind(self.addr) {
                Ok(()) => break sock.listen(16).expect("listen"),
                Err(e) if tries < 50 => {
                    tries += 1;
                    let _ = e;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => panic!("rebind {}: {e}", self.addr),
            }
        };
        self.serve(listener);
    }
}

impl Drop for FakeRigctld {
    fn drop(&mut self) {
        self.kill();
    }
}

async fn client(sock: tokio::net::TcpStream, state: Arc<Mutex<State>>) {
    let (r, mut w) = sock.into_split();
    let mut lines = BufReader::new(r).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim_end().to_string();
        let (reply, delay) = {
            let mut s = state.lock().unwrap();
            s.log.push(line.clone());
            (answer(&mut s, &line), s.delay)
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if w.write_all(reply.as_bytes()).await.is_err() {
            return;
        }
    }
}

fn answer(s: &mut State, line: &str) -> String {
    let body = line.strip_prefix('+').unwrap_or(line);
    let mut it = body.split_whitespace();
    let cmd = it.next().unwrap_or("");
    let args: Vec<&str> = it.collect();
    let ok = |name: &str| format!("{name}: {}\nRPRT 0\n", args.join(" "));
    match cmd {
        "f" => format!("get_freq:\nFrequency: {}\nRPRT 0\n", s.freq),
        "F" => match args.first().and_then(|a| a.parse().ok()) {
            Some(hz) => {
                s.freq = hz;
                ok("set_freq")
            }
            None => "set_freq:\nRPRT -1\n".into(),
        },
        "m" => format!("get_mode:\nMode: {}\nPassband: {}\nRPRT 0\n", s.mode, s.passband),
        "M" => match (args.first(), args.get(1).and_then(|p| p.parse().ok())) {
            (Some(m), Some(pb)) => {
                s.mode = m.to_string();
                s.passband = pb;
                ok("set_mode")
            }
            _ => "set_mode:\nRPRT -1\n".into(),
        },
        "v" => format!("get_vfo:\nVFO: {}\nRPRT 0\n", s.vfo),
        "V" => match args.first() {
            Some(&v @ ("VFOA" | "VFOB")) => {
                s.vfo = v.to_string();
                ok("set_vfo")
            }
            _ => format!("set_vfo: {}\nRPRT -1\n", args.join(" ")),
        },
        "s" => format!("get_split_vfo:\nSplit: {}\nTX VFO: {}\nRPRT 0\n", u8::from(s.split), s.tx_vfo),
        "S" if s.caps.can_split => {
            s.split = args.first() == Some(&"1");
            s.tx_vfo = args.get(1).unwrap_or(&"None").to_string();
            ok("set_split_vfo")
        }
        "S" => "set_split_vfo:\nRPRT -11\n".into(),
        "t" => match s.ptt_error {
            Some(code) => format!("get_ptt:\nRPRT {code}\n"),
            None => format!("get_ptt:\nPTT: {}\nRPRT 0\n", u8::from(s.ptt)),
        },
        "T" => {
            s.ptt = args.first() != Some(&"0");
            ok("set_ptt")
        }
        "l" => {
            let name = args.first().copied().unwrap_or("");
            match Level::ALL.iter().find(|l| l.token() == name) {
                Some(l) if s.caps.get_levels.contains(l) => {
                    format!("get_level: {name}\n{}\nRPRT 0\n", s.levels.get(l.token()).copied().unwrap_or(0.0))
                }
                _ => format!("get_level: {name}\nRPRT -11\n"),
            }
        }
        "L" => {
            let name = args.first().copied().unwrap_or("");
            match (Level::ALL.iter().find(|l| l.token() == name), args.get(1).and_then(|v| v.parse::<f32>().ok())) {
                (Some(l), Some(v)) if s.caps.set_levels.contains(l) => {
                    s.levels.insert(l.token(), v);
                    ok("set_level")
                }
                _ => format!("set_level: {}\nRPRT -11\n", args.join(" ")),
            }
        }
        "\\dump_caps" => dump_caps(&s.caps),
        _ => format!("{cmd}:\nRPRT -4\n"),
    }
}

fn dump_caps(c: &RigCaps) -> String {
    let yn = |b: bool| if b { "Y" } else { "N" };
    let levels = |ls: &[Level]| ls.iter().map(|l| format!("{}(0..1/0.01) ", l.token())).collect::<String>();
    format!(
        "dump_caps:\nCaps dump for model: 1\nModel name:\t{}\nMfg name:\tHamlib\nGet level: {}\nSet level: {}\nMode list: {}\nCan set Frequency:\t{}\nCan get Frequency:\tY\nCan set Mode:\t{}\nCan get Mode:\tY\nCan set VFO:\t{}\nCan get VFO:\tY\nCan set PTT:\tY\nCan get PTT:\t{}\nCan set Split VFO:\t{}\nCan get Split VFO:\tY\nRPRT 0\n",
        c.model,
        levels(&c.get_levels),
        levels(&c.set_levels),
        c.modes.join(" "),
        yn(c.can_set_freq),
        yn(c.can_set_mode),
        yn(c.can_set_vfo),
        yn(c.can_get_ptt),
        yn(c.can_split),
    )
}
