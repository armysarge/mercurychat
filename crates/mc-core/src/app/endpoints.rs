//! SET-03's read-only check of My own Mercury: each endpoint is connected to
//! and closed again, nothing sent, and named with what went wrong in words.

use crate::profile::ModemTarget;
use serde::Serialize;
use std::time::Duration;
use ts_rs::TS;

const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EndpointCheck {
    /// `control`, `data`, `broadcast` or `telemetry`.
    pub name: String,
    /// `host:port` as connected to.
    pub address: String,
    pub ok: bool,
    pub detail: String,
}

fn address(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// The host and port of a `ws://` or `wss://` URL (default 80 or 443).
fn ws_endpoint(url: &str) -> Option<(String, u16)> {
    let (rest, default) = match url.strip_prefix("ws://") {
        Some(r) => (r, 80),
        None => (url.strip_prefix("wss://")?, 443),
    };
    let authority = rest.split('/').next().unwrap_or("");
    if let Some(v6) = authority.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => default,
        };
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None if !authority.is_empty() => Some((authority.to_string(), default)),
        None => None,
    }
}

async fn one(name: &str, host: &str, port: u16) -> EndpointCheck {
    let addr = address(host, port);
    let (ok, detail) = match tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => (true, "Answers".to_string()),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            (false, "Nothing answers on this port: is Mercury running, and is this its port?".to_string())
        }
        Ok(Err(e)) => (false, format!("Could not connect: {e}")),
        Err(_) => (false, format!("No answer within {} s: check the host and any firewall", TIMEOUT.as_secs())),
    };
    EndpointCheck { name: name.into(), address: addr, ok, detail }
}

/// Check My own Mercury's endpoints, in order: control, data, then the
/// optional broadcast port and telemetry link. `None` for other modems.
pub async fn check(target: &ModemTarget) -> Option<Vec<EndpointCheck>> {
    let ModemTarget::Existing { host, control_port, data_port, broadcast_port, telemetry_url } = target else {
        return None;
    };
    let host = host.trim();
    let mut out = vec![one("control", host, *control_port).await, one("data", host, *data_port).await];
    if let Some(p) = broadcast_port {
        out.push(one("broadcast", host, *p).await);
    }
    if let Some(url) = telemetry_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        out.push(match ws_endpoint(url) {
            Some((h, p)) => one("telemetry", &h, p).await,
            None => EndpointCheck {
                name: "telemetry".into(),
                address: url.into(),
                ok: false,
                detail: "Not a ws:// address".into(),
            },
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::ws_endpoint;

    #[test]
    fn ws_endpoints() {
        assert_eq!(ws_endpoint("ws://127.0.0.1:10000/websocket"), Some(("127.0.0.1".into(), 10000)));
        assert_eq!(ws_endpoint("ws://[::1]:10000/websocket"), Some(("::1".into(), 10000)));
        assert_eq!(ws_endpoint("wss://example.org/x"), Some(("example.org".into(), 443)));
        assert_eq!(ws_endpoint("http://x"), None);
    }
}
