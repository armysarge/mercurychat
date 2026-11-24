//! The CAT engine as a rigctld (M5 Task 11e): it speaks the part of rigctld's
//! protocol that Mercury and the app use, and turns each request into the
//! radio's own commands from a CAT commands file, sent over a [`Port`].
//! Mercury keys the radio through it exactly as it would through Hamlib, and
//! the app reads and sets the frequency through the same path.
//!
//! Nothing is made up: a mode is reported only once it has been set, and
//! anything the file cannot do answers with an error code.

use super::file::Radio;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Hamlib's own reply to `\dump_state` for a dummy rig (4.7.2). Mercury asks
/// for it when it connects, so it has to be the real thing.
const DUMP_STATE: &str = include_str!("dump_state.txt");

/// What to expect back after a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// Nothing to read (the radio may still acknowledge; that is discarded).
    None,
    /// At least this many bytes, and whatever else follows at once (a radio
    /// that echoes the command sends the echo first).
    Bytes(usize),
}

/// A serial line to a radio.
pub trait Port: Send {
    /// Discard anything received and not yet read, send `cmd`, read `reply`.
    fn transact(&mut self, cmd: &[u8], reply: Reply) -> io::Result<Vec<u8>>;
}

/// The radio on a [`Port`], with what is known of it.
pub struct Engine {
    radio: Radio,
    port: Box<dyn Port>,
    mode: Option<(String, i32)>,
    ptt: bool,
}

/// rigctld's error numbers.
const EINVAL: i32 = -1;
const EIO: i32 = -6;
const ENAVAIL: i32 = -11;
const EPROTO: i32 = -8;

impl Engine {
    /// Sends the radio's start commands.
    pub fn new(radio: Radio, port: Box<dyn Port>) -> Engine {
        let mut e = Engine { radio, port, mode: None, ptt: false };
        let start = e.radio.start.clone();
        for c in &start {
            let _ = e.port.transact(c, Reply::None);
        }
        e
    }

    pub fn radio(&self) -> &Radio {
        &self.radio
    }

    fn send_all(&mut self, cmds: &[Vec<u8>]) -> Result<(), i32> {
        for c in cmds {
            self.port.transact(c, Reply::None).map_err(|_| EIO)?;
        }
        Ok(())
    }

    /// The radio's frequency, asked of the radio.
    pub fn get_freq(&mut self) -> Result<u64, i32> {
        let want = if self.radio.text { self.radio.read_len } else { self.radio.read_len / 2 };
        let cmd = self.radio.read_freq.clone();
        let reply = self.port.transact(&cmd, Reply::Bytes(want)).map_err(|_| EIO)?;
        self.radio.parse_freq_reply(&reply).map_err(|_| EPROTO)
    }

    pub fn set_freq(&mut self, hz: u64) -> Result<(), i32> {
        let bytes = self.radio.set_freq_bytes(hz).map_err(|_| EINVAL)?;
        self.port.transact(&bytes, Reply::None).map_err(|_| EIO)?;
        Ok(())
    }

    pub fn set_ptt(&mut self, on: bool) -> Result<(), i32> {
        let cmds = if on { self.radio.ptt_on.clone() } else { self.radio.ptt_off.clone() };
        if cmds.is_empty() {
            return Err(ENAVAIL);
        }
        self.send_all(&cmds)?;
        self.ptt = on;
        Ok(())
    }

    fn set_mode(&mut self, mode: &str, passband: i32) -> Result<(), i32> {
        let cmds = self.radio.modes.get(mode).cloned().ok_or(ENAVAIL)?;
        self.send_all(&cmds)?;
        self.mode = Some((mode.to_string(), passband));
        Ok(())
    }

    /// The reply to one request line. `None` closes the connection.
    pub fn answer(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        let (ext, body) = match line.strip_prefix('+') {
            Some(r) => (true, r),
            None => (false, line),
        };
        let mut it = body.split_whitespace();
        let cmd = it.next().unwrap_or("");
        let args: Vec<&str> = it.collect();
        // A command's reply: the name line and the fields, or the bare values.
        let ok = |name: &str, fields: &[(&str, String)]| -> String {
            if ext {
                let mut s = format!("{name}:\n");
                for (k, v) in fields {
                    s.push_str(&format!("{k}: {v}\n"));
                }
                s.push_str("RPRT 0\n");
                s
            } else if fields.is_empty() {
                "RPRT 0\n".into()
            } else {
                fields.iter().map(|(_, v)| format!("{v}\n")).collect()
            }
        };
        let err =
            |name: &str, code: i32| if ext { format!("{name}:\nRPRT {code}\n") } else { format!("RPRT {code}\n") };
        Some(match cmd {
            "q" | "Q" | "\\quit" => return None,
            "f" | "\\get_freq" => match self.get_freq() {
                Ok(hz) => ok("get_freq", &[("Frequency", hz.to_string())]),
                Err(c) => err("get_freq", c),
            },
            "F" | "\\set_freq" => match args.first().and_then(|a| a.parse::<f64>().ok()).filter(|h| *h >= 0.0) {
                Some(hz) => match self.set_freq(hz.round() as u64) {
                    Ok(()) => ok("set_freq", &[]),
                    Err(c) => err("set_freq", c),
                },
                None => err("set_freq", EINVAL),
            },
            "m" | "\\get_mode" => match &self.mode {
                Some((m, pb)) => ok("get_mode", &[("Mode", m.clone()), ("Passband", pb.to_string())]),
                None => err("get_mode", ENAVAIL),
            },
            "M" | "\\set_mode" => match (args.first(), args.get(1).and_then(|p| p.parse::<i32>().ok())) {
                (Some(m), Some(pb)) => match self.set_mode(m, pb) {
                    Ok(()) => ok("set_mode", &[]),
                    Err(c) => err("set_mode", c),
                },
                _ => err("set_mode", EINVAL),
            },
            "t" | "\\get_ptt" => ok("get_ptt", &[("PTT", u8::from(self.ptt).to_string())]),
            "T" | "\\set_ptt" => match args.first() {
                Some(&v @ ("0" | "1" | "2" | "3")) => match self.set_ptt(v != "0") {
                    Ok(()) => ok("set_ptt", &[]),
                    Err(c) => err("set_ptt", c),
                },
                _ => err("set_ptt", EINVAL),
            },
            "v" | "\\get_vfo" => ok("get_vfo", &[("VFO", "VFOA".into())]),
            "V" | "\\set_vfo" => match args.first() {
                Some(&"VFOA") => ok("set_vfo", &[]),
                _ => err("set_vfo", ENAVAIL),
            },
            "s" | "\\get_split_vfo" => ok("get_split_vfo", &[("Split", "0".into()), ("TX VFO", "VFOA".into())]),
            "S" | "\\set_split_vfo" => match args.first() {
                Some(&"0") => ok("set_split_vfo", &[]),
                _ => err("set_split_vfo", ENAVAIL),
            },
            "l" | "\\get_level" => err("get_level", ENAVAIL),
            "L" | "\\set_level" => err("set_level", ENAVAIL),
            "\\chk_vfo" | "chk_vfo" => ok("chk_vfo", &[("ChkVFO", "0".into())]),
            "\\dump_state" => DUMP_STATE.replace("\r\n", "\n"),
            "\\dump_caps" => self.dump_caps(),
            other => err(other.trim_start_matches('\\'), -4),
        })
    }

    fn dump_caps(&self) -> String {
        let modes: Vec<&str> = self.radio.modes.keys().map(String::as_str).collect();
        format!(
            "Caps dump for model: 1\nModel name:\t{}\nMfg name:\tMercuryChat\nMode list: {}\nCan set Frequency:\tY\nCan get Frequency:\tY\nCan set Mode:\t{}\nCan get Mode:\tN\nCan set VFO:\tN\nCan get VFO:\tY\nCan set PTT:\tY\nCan get PTT:\tY\nCan set Split VFO:\tN\nCan get Split VFO:\tY\nRPRT 0\n",
            self.radio.name,
            modes.join(" "),
            if modes.is_empty() { "N" } else { "Y" },
        )
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // Hand the radio back as it was found: unkeyed, and with its exit commands.
        let exit = self.radio.exit.clone();
        let off = self.radio.ptt_off.clone();
        if self.ptt {
            let _ = self.send_all(&off);
        }
        let _ = self.send_all(&exit);
    }
}

/// The engine listening for rigctld clients.
pub struct CatServer {
    pub addr: SocketAddr,
    engine: Arc<Mutex<Engine>>,
    /// The accept loop and every client: all end with the server, so the
    /// radio's port is free again once it is dropped.
    tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl CatServer {
    /// Listen at `addr` (port 0 picks one). Must run inside a tokio runtime.
    pub async fn start(addr: SocketAddr, engine: Engine) -> io::Result<CatServer> {
        let listener = TcpListener::bind(addr).await?;
        let addr = listener.local_addr()?;
        let engine = Arc::new(Mutex::new(engine));
        let e = engine.clone();
        let tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::default();
        let t = tasks.clone();
        let accept = tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                let conn = tokio::spawn(client(sock, e.clone()));
                let mut t = t.lock().unwrap();
                t.retain(|h| !h.is_finished());
                t.push(conn);
            }
        });
        tasks.lock().unwrap().push(accept);
        Ok(CatServer { addr, engine, tasks })
    }

    /// The radio's frequency, read through the engine (the app's own reads
    /// share the line with Mercury's).
    pub async fn freq(&self) -> Result<u64, i32> {
        let e = self.engine.clone();
        tokio::task::spawn_blocking(move || e.lock().unwrap().get_freq()).await.map_err(|_| EIO)?
    }

    pub async fn set_freq(&self, hz: u64) -> Result<(), i32> {
        let e = self.engine.clone();
        tokio::task::spawn_blocking(move || e.lock().unwrap().set_freq(hz)).await.map_err(|_| EIO)?
    }
}

impl Drop for CatServer {
    fn drop(&mut self) {
        for t in self.tasks.lock().unwrap().drain(..) {
            t.abort();
        }
    }
}

/// The longest request line: a rigctld command is a few dozen characters.
const MAX_LINE: u64 = 4096;
const NL: char = '\n';

async fn client(sock: tokio::net::TcpStream, engine: Arc<Mutex<Engine>>) {
    let (r, mut w) = sock.into_split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        // A line that does not end within the limit is not a rigctld client.
        match (&mut reader).take(MAX_LINE).read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) if !line.ends_with(NL) => return,
            Ok(_) => {}
        }
        let (e, l) = (engine.clone(), line.clone());
        match tokio::task::spawn_blocking(move || e.lock().unwrap().answer(&l)).await {
            Ok(Some(text)) => {
                if w.write_all(text.as_bytes()).await.is_err() {
                    return;
                }
            }
            _ => return,
        }
    }
}

/// A radio that answers like the real one, for tests (no serial port needed).
#[cfg(any(test, feature = "fake"))]
pub mod fake {
    use super::*;
    use crate::cat::codec::{bytes_to_hex, decode_freq, encode_freq, hex_to_bytes};

    /// What the radio did, shared with the test.
    #[derive(Default, Debug)]
    pub struct Log {
        pub sent: Vec<Vec<u8>>,
        pub freq: u64,
        pub keyed: bool,
    }

    /// A radio defined by a [`Radio`], answering frequency reads with
    /// `echo` (if the radio echoes) then `prefix`, the digits and `suffix`.
    pub struct FakeRadio {
        radio: Radio,
        prefix: String,
        suffix: String,
        echo: bool,
        pub log: Arc<Mutex<Log>>,
    }

    impl FakeRadio {
        pub fn new(radio: Radio, prefix: &str, suffix: &str, echo: bool) -> (FakeRadio, Arc<Mutex<Log>>) {
            let log = Arc::new(Mutex::new(Log { freq: 14_074_000, ..Log::default() }));
            (FakeRadio { radio, prefix: prefix.into(), suffix: suffix.into(), echo, log: log.clone() }, log)
        }

        fn chars(&self, b: &[u8]) -> String {
            if self.radio.text {
                String::from_utf8_lossy(b).into_owned()
            } else {
                bytes_to_hex(b)
            }
        }
    }

    impl Port for FakeRadio {
        fn transact(&mut self, cmd: &[u8], reply: Reply) -> io::Result<Vec<u8>> {
            let mut log = self.log.lock().unwrap();
            log.sent.push(cmd.to_vec());
            let sent = self.chars(cmd);
            if self.radio.ptt_on.iter().any(|c| c == cmd) {
                log.keyed = true;
            }
            if self.radio.ptt_off.iter().any(|c| c == cmd) {
                log.keyed = false;
            }
            // A set-frequency command: the template around {}.
            if let Some((pre, post)) = self.radio.set_freq.split_once("{}") {
                if sent.starts_with(pre) && sent.ends_with(post) && sent.len() > pre.len() + post.len() {
                    let digits = &sent[pre.len()..sent.len() - post.len()];
                    if let Ok(hz) = decode_freq(self.radio.freq_coding, digits, self.radio.freq_step_hz) {
                        log.freq = hz;
                    }
                }
            }
            if let Reply::Bytes(_) = reply {
                let digits =
                    encode_freq(self.radio.read_coding, log.freq, self.radio.read_step_hz, self.radio.read_digits)
                        .unwrap();
                let body = format!("{}{digits}{}", self.prefix, self.suffix);
                let mut out = Vec::new();
                if self.echo {
                    out.extend_from_slice(cmd);
                }
                if self.radio.text {
                    out.extend_from_slice(body.as_bytes());
                } else {
                    out.extend(hex_to_bytes(&body).unwrap());
                }
                return Ok(out);
            }
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeRadio;
    use super::*;
    use crate::cat::file::builtin;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;

    fn radio(name: &str) -> Radio {
        builtin().into_iter().find(|r| r.name == name).unwrap()
    }

    fn icom() -> (Engine, Arc<Mutex<super::fake::Log>>) {
        let (fake, log) = FakeRadio::new(radio("Icom IC-7300"), "FEFEE09403", "FD", true);
        (Engine::new(radio("Icom IC-7300"), Box::new(fake)), log)
    }

    fn yaesu() -> (Engine, Arc<Mutex<super::fake::Log>>) {
        let (fake, log) = FakeRadio::new(radio("Yaesu FT-991A"), "FA", ";", false);
        (Engine::new(radio("Yaesu FT-991A"), Box::new(fake)), log)
    }

    #[test]
    fn the_frequency_is_set_and_read_through_the_radios_own_commands() {
        let (mut e, log) = icom();
        assert_eq!(e.answer("F 7105000").unwrap(), "RPRT 0\n");
        assert_eq!(log.lock().unwrap().freq, 7_105_000);
        assert_eq!(log.lock().unwrap().sent[0], hex_bytes("FEFE94E0050050100700FD"));
        assert_eq!(e.answer("f").unwrap(), "7105000\n", "read back through the echo");
        assert_eq!(e.answer("+f").unwrap(), "get_freq:\nFrequency: 7105000\nRPRT 0\n");
    }

    fn hex_bytes(s: &str) -> Vec<u8> {
        crate::cat::codec::hex_to_bytes(s).unwrap()
    }

    #[test]
    fn a_text_radio_works_the_same_way() {
        let (mut e, log) = yaesu();
        e.answer("F 14074000");
        assert_eq!(log.lock().unwrap().sent[0], b"FA014074000;".to_vec());
        assert_eq!(e.answer("f").unwrap(), "14074000\n");
    }

    #[test]
    fn ptt_keys_and_unkeys_the_radio() {
        let (mut e, log) = icom();
        assert_eq!(e.answer("T 1").unwrap(), "RPRT 0\n");
        assert!(log.lock().unwrap().keyed);
        assert_eq!(e.answer("t").unwrap(), "1\n");
        assert_eq!(e.answer("T 0").unwrap(), "RPRT 0\n");
        assert!(!log.lock().unwrap().keyed);
        assert_eq!(e.answer("T x").unwrap(), "RPRT -1\n");
    }

    #[test]
    fn dropping_the_engine_unkeys_a_radio_left_transmitting() {
        let (mut e, log) = icom();
        e.answer("T 1");
        drop(e);
        assert!(!log.lock().unwrap().keyed);
    }

    #[test]
    fn the_mode_is_reported_only_once_it_has_been_set() {
        let (mut e, log) = icom();
        assert_eq!(e.answer("m").unwrap(), "RPRT -11\n", "nothing is made up");
        assert_eq!(e.answer("M PKTUSB 3000").unwrap(), "RPRT 0\n");
        assert_eq!(log.lock().unwrap().sent.last().unwrap(), &hex_bytes("FEFE94E02600010101FD"));
        assert_eq!(e.answer("m").unwrap(), "PKTUSB\n3000\n");
        assert_eq!(e.answer("M RTTY 500").unwrap(), "RPRT -11\n", "a mode the file lacks");
    }

    #[test]
    fn what_the_radio_cannot_do_is_an_error_code() {
        let (mut e, _) = icom();
        assert_eq!(e.answer("l RFPOWER").unwrap(), "RPRT -11\n");
        assert_eq!(e.answer("+l RFPOWER").unwrap(), "get_level:\nRPRT -11\n");
        assert_eq!(e.answer("S 1 VFOB").unwrap(), "RPRT -11\n");
        assert_eq!(e.answer("zz").unwrap(), "RPRT -4\n");
        assert_eq!(e.answer("q"), None);
    }

    #[test]
    fn dump_state_is_hamlibs_own() {
        let (mut e, _) = icom();
        let s = e.answer("\\dump_state").unwrap();
        assert!(s.starts_with("1\n1\n0\n"), "{s:?}");
        assert!(!s.contains('\r'));
        assert_eq!(e.answer("\\chk_vfo").unwrap(), "0\n");
    }

    #[test]
    fn a_dead_radio_is_an_io_error() {
        struct Dead;
        impl Port for Dead {
            fn transact(&mut self, _: &[u8], _: Reply) -> io::Result<Vec<u8>> {
                Err(io::Error::other("unplugged"))
            }
        }
        let mut e = Engine::new(radio("Icom IC-7300"), Box::new(Dead));
        assert_eq!(e.answer("f").unwrap(), "RPRT -6\n");
        assert_eq!(e.answer("T 1").unwrap(), "RPRT -6\n");
    }

    #[tokio::test]
    async fn dropping_the_server_ends_its_clients_and_frees_the_radio() {
        let (e, log) = icom();
        let server = CatServer::start("127.0.0.1:0".parse().unwrap(), e).await.unwrap();
        let mut c = TcpStream::connect(server.addr).await.unwrap();
        c.write_all(
            b"T 1
",
        )
        .await
        .unwrap();
        let mut buf = [0u8; 16];
        let _ = c.read(&mut buf).await.unwrap();
        assert!(log.lock().unwrap().keyed);
        drop(server);
        // The client is dropped with it: the read ends, and the radio is unkeyed.
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), c.read(&mut buf))
            .await
            .expect("closed")
            .unwrap_or(0);
        assert_eq!(n, 0);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!log.lock().unwrap().keyed, "the engine went with the server and unkeyed the radio");
    }

    #[tokio::test]
    async fn a_line_that_never_ends_closes_the_connection() {
        let (e, _) = icom();
        let server = CatServer::start("127.0.0.1:0".parse().unwrap(), e).await.unwrap();
        let mut c = TcpStream::connect(server.addr).await.unwrap();
        c.write_all(&vec![b'x'; 64 * 1024]).await.unwrap();
        let mut buf = [0u8; 16];
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), c.read(&mut buf))
            .await
            .expect("closed, not waiting for more")
            .unwrap_or(0);
        assert_eq!(n, 0);
        // A new client is still served.
        let mut d = TcpStream::connect(server.addr).await.unwrap();
        d.write_all(
            b"f
",
        )
        .await
        .unwrap();
        let n = d.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("14074000"));
    }

    #[tokio::test]
    async fn a_client_over_tcp_gets_the_same_answers() {
        let (e, log) = icom();
        let server = CatServer::start("127.0.0.1:0".parse().unwrap(), e).await.unwrap();
        let mut c = TcpStream::connect(server.addr).await.unwrap();
        c.write_all(b"+F 10136000\n+f\n").await.unwrap();
        let mut got = String::new();
        let mut buf = [0u8; 256];
        while !got.contains("Frequency") {
            let n = c.read(&mut buf).await.unwrap();
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        assert!(got.contains("set_freq:\nRPRT 0\n") && got.contains("Frequency: 10136000"), "{got:?}");
        assert_eq!(log.lock().unwrap().freq, 10_136_000);
        assert_eq!(server.freq().await.unwrap(), 10_136_000, "the app reads through the same line");
    }
}
