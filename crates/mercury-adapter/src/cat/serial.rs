//! The radio's serial line (M5 Task 11e) as a [`Port`].

use super::server::{Port, Reply};
use serialport::{ClearBuffer, SerialPort};
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// How long to wait for a reply, and for the rest of one once it has begun.
const REPLY_WAIT: Duration = Duration::from_millis(600);
const IDLE_GAP: Duration = Duration::from_millis(40);

pub struct SerialRadio {
    port: Box<dyn SerialPort>,
}

impl SerialRadio {
    /// Open `path` (`COM3`, `/dev/ttyUSB0`) at `baud`, 8N1 with no flow control.
    pub fn open(path: &str, baud: u32) -> io::Result<SerialRadio> {
        let port = serialport::new(path, baud)
            .timeout(Duration::from_millis(20))
            .open()
            .map_err(|e| io::Error::other(format!("{path}: {e}")))?;
        Ok(SerialRadio { port })
    }
}

impl Port for SerialRadio {
    fn transact(&mut self, cmd: &[u8], reply: Reply) -> io::Result<Vec<u8>> {
        self.port.clear(ClearBuffer::Input).map_err(io::Error::other)?;
        self.port.write_all(cmd)?;
        self.port.flush()?;
        let want = match reply {
            Reply::None => return Ok(vec![]),
            Reply::Bytes(n) => n,
        };
        let mut got = Vec::new();
        let mut buf = [0u8; 64];
        let start = Instant::now();
        let mut last = Instant::now();
        loop {
            match self.port.read(&mut buf) {
                Ok(n) if n > 0 => {
                    got.extend_from_slice(&buf[..n]);
                    last = Instant::now();
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e),
            }
            // Enough, and quiet: the echo (if any) and the reply are both in.
            if got.len() >= want && last.elapsed() >= IDLE_GAP {
                return Ok(got);
            }
            if start.elapsed() >= REPLY_WAIT {
                return if got.len() >= want {
                    Ok(got)
                } else {
                    Err(io::Error::new(io::ErrorKind::TimedOut, "the radio did not answer"))
                };
            }
        }
    }
}
