//! A built-in modem's ports (M5 §5.1): all free on 127.0.0.1, or the taken one
//! named with a set that is free. A listener already there is never adopted.

use crate::profile::BuiltInPorts;

/// A port is taken: which one, and a free set to offer instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortTaken {
    pub port: u16,
    pub suggestion: BuiltInPorts,
}

fn free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Ok when every port is free; else the first taken one and the next set
/// that is free throughout (control, broadcast and UI step by 10, rigctld by 1).
pub fn check_free(p: &BuiltInPorts) -> Result<(), PortTaken> {
    let Some(port) = p.all().into_iter().map(|(_, port)| port).find(|&port| !free(port)) else {
        return Ok(());
    };
    let suggestion = (1..=200u16)
        .filter_map(|n| {
            Some(BuiltInPorts {
                control: p.control.checked_add(n * 10)?,
                broadcast: p.broadcast.checked_add(n * 10)?,
                ui: p.ui.checked_add(n * 10)?,
                rigctld: p.rigctld.checked_add(n)?,
            })
        })
        .find(|s| s.control < u16::MAX && s.all().into_iter().all(|(_, port)| free(port)))
        .unwrap_or(*p);
    Err(PortTaken { port, suggestion })
}
