//! A fake FLRig XML-RPC server for tests (M3b.1 Task 4): frequency, mode
//! and PTT, set by the test or by the client, faults on request, and a log
//! of every call as received.

use crate::flrig::{xml_escape, xml_unescape};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpSocket};
use tokio::task::JoinHandle;

struct State {
    freq: u64,
    mode: String,
    ptt: bool,
    faults: HashMap<String, String>,
    log: Vec<(String, String)>,
    tasks: Vec<JoinHandle<()>>,
}

pub struct FakeFlrig {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl FakeFlrig {
    pub async fn start() -> FakeFlrig {
        let state = Arc::new(Mutex::new(State {
            freq: 14_074_000,
            mode: "USB".into(),
            ptt: false,
            faults: HashMap::new(),
            log: vec![],
            tasks: vec![],
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake flrig");
        let addr = listener.local_addr().unwrap();
        let fake = FakeFlrig { addr, state };
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

    pub fn freq(&self) -> u64 {
        self.state.lock().unwrap().freq
    }
    pub fn set_freq(&self, hz: u64) {
        self.state.lock().unwrap().freq = hz;
    }
    pub fn mode(&self) -> String {
        self.state.lock().unwrap().mode.clone()
    }
    pub fn set_mode(&self, mode: &str) {
        self.state.lock().unwrap().mode = mode.into();
    }
    /// The radio keyed (or not) by someone else: Mercury, in the real setup.
    pub fn set_ptt(&self, on: bool) {
        self.state.lock().unwrap().ptt = on;
    }
    /// Answer `method` with an XML-RPC fault carrying `message`.
    pub fn fault_on(&self, method: &str, message: &str) {
        self.state.lock().unwrap().faults.insert(method.into(), message.into());
    }
    /// Every call received: (method, its `<params>` as sent), oldest first.
    pub fn log(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().log.clone()
    }
    /// Stop listening and drop every client, as a closed FLRig would.
    pub fn kill(&self) {
        for t in self.state.lock().unwrap().tasks.drain(..) {
            t.abort();
        }
    }
    /// Listen again on the same port; the radio keeps its state.
    pub async fn restart(&self) {
        let mut tries = 0;
        let listener = loop {
            let sock = TcpSocket::new_v4().unwrap();
            let _ = sock.set_reuseaddr(true);
            match sock.bind(self.addr) {
                Ok(()) => break sock.listen(16).expect("listen"),
                Err(e) if tries < 50 => {
                    tries += 1;
                    let _ = e;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => panic!("fake flrig restart: {e}"),
            }
        };
        self.serve(listener);
    }
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let a = s.find(open)? + open.len();
    let b = s[a..].find(close)? + a;
    Some(&s[a..b])
}

fn value(v: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><methodResponse><params><param><value>{v}</value></param></params></methodResponse>"
    )
}

fn fault(msg: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?><methodResponse><fault><value><struct><member><name>faultCode</name><value><int>1</int></value></member><member><name>faultString</name><value><string>{}</string></value></member></struct></value></fault></methodResponse>",
        xml_escape(msg)
    )
}

fn answer(s: &mut State, method: &str, params: &str) -> String {
    if let Some(msg) = s.faults.get(method) {
        return fault(msg);
    }
    let arg = between(params, "<value>", "</value>")
        .map(|v| {
            let v = v.trim();
            let inner = v
                .strip_prefix('<')
                .and_then(|r| r.find('>').map(|e| &r[..e]))
                .and_then(|tag| between(v, &format!("<{tag}>"), &format!("</{tag}>")));
            xml_unescape(inner.unwrap_or(v))
        })
        .unwrap_or_default();
    match method {
        "rig.get_vfo" => value(&format!("<string>{}</string>", s.freq)),
        "rig.get_mode" => value(&format!("<string>{}</string>", xml_escape(&s.mode))),
        "rig.get_ptt" => value(&format!("<int>{}</int>", u8::from(s.ptt))),
        "rig.set_frequency" => match arg.parse::<f64>() {
            Ok(f) => {
                s.freq = f.round() as u64;
                value("<i4>0</i4>")
            }
            Err(_) => fault("bad frequency"),
        },
        "rig.set_mode" => {
            s.mode = arg;
            value("<i4>0</i4>")
        }
        other => fault(&format!("unknown method {other}")),
    }
}

async fn client(mut sock: tokio::net::TcpStream, state: Arc<Mutex<State>>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Headers, then Content-Length bytes of body.
    let body = loop {
        let Ok(n) = sock.read(&mut chunk).await else { return };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let len = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_string()))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= len {
                break body.to_string();
            }
        }
    };
    let method = between(&body, "<methodName>", "</methodName>").unwrap_or("").to_string();
    let params = between(&body, "<params>", "</params>").unwrap_or("").to_string();
    let reply = {
        let mut s = state.lock().unwrap();
        s.log.push((method.clone(), params.clone()));
        answer(&mut s, &method, &params)
    };
    let http = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    let _ = sock.write_all(http.as_bytes()).await;
    let _ = sock.shutdown().await;
}
