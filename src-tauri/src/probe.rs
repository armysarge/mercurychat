//! Limits on the read-only modem probe (docs/02 §7, MOD-02): the window may
//! only point it at this machine or at an endpoint a station profile already
//! names, and it takes that endpoint's lock for as long as it runs.

use mc_core::lock::{lock_endpoint, LockFile};
use mc_core::profile::{ModemTarget, StationProfile};
use std::net::IpAddr;
use std::path::Path;

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// This machine, or exactly an endpoint a station profile already names.
pub fn probe_target_allowed(host: &str, port: u16, profiles: &[StationProfile]) -> Result<(), String> {
    if is_loopback(host) {
        return Ok(());
    }
    let named = profiles.iter().any(|p| {
        matches!(&p.modem, ModemTarget::Existing { host: h, control_port, .. }
            if h.eq_ignore_ascii_case(host) && *control_port == port)
    });
    if named {
        Ok(())
    } else {
        Err(format!(
            "{host}:{port} is not this computer or a modem named in a station profile; add it to a profile first"
        ))
    }
}

/// Check the target and hold its endpoint lock while the probe runs.
pub fn probe_guard(locks_dir: &Path, host: &str, port: u16, profiles: &[StationProfile]) -> Result<LockFile, String> {
    probe_target_allowed(host, port, profiles)?;
    lock_endpoint(locks_dir, host, port).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::profile::{ModemTarget, StationProfile};

    fn profile_at(host: &str, port: u16) -> StationProfile {
        let mut p = StationProfile::new_existing("ZS6ABC");
        p.modem = ModemTarget::Existing {
            host: host.into(),
            control_port: port,
            data_port: port + 1,
            broadcast_port: None,
            telemetry_url: None,
        };
        p
    }

    #[test]
    fn probe_refuses_a_remote_host() {
        let e = probe_target_allowed("10.0.0.5", 8300, &[]).unwrap_err();
        assert!(e.contains("10.0.0.5"), "{e}");
        assert!(probe_target_allowed("example.org", 8300, &[]).is_err());
    }

    #[test]
    fn probe_allows_loopback_and_profile_endpoints() {
        for host in ["127.0.0.1", "::1", "localhost", "127.0.0.2"] {
            assert!(probe_target_allowed(host, 8300, &[]).is_ok(), "{host}");
        }
        let profiles = [profile_at("192.168.1.20", 8300)];
        assert!(probe_target_allowed("192.168.1.20", 8300, &profiles).is_ok());
        assert!(probe_target_allowed("192.168.1.20", 8400, &profiles).is_err(), "another port on that host");
    }

    #[test]
    fn probe_respects_the_endpoint_lock() {
        let dir = tempfile::tempdir().unwrap();
        let held = mc_core::lock::lock_endpoint(dir.path(), "127.0.0.1", 8300).unwrap();
        let e = probe_guard(dir.path(), "127.0.0.1", 8300, &[]).unwrap_err();
        assert!(e.contains("another MercuryChat instance"), "{e}");
        drop(held);
        let guard = probe_guard(dir.path(), "127.0.0.1", 8300, &[]).unwrap();
        assert!(mc_core::lock::lock_endpoint(dir.path(), "127.0.0.1", 8300).is_err(), "held while probing");
        drop(guard);
    }
}
