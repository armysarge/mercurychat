//! M5 Tasks 3–5: the built-in modem (ports, the supervisor, the service).

use mc_core::app::builtin::ports::{check_free, PortTaken};
use mc_core::profile::BuiltInPorts;
use std::net::TcpListener;

/// M5 §5.1: a taken port is named, with a set that is free throughout.
#[test]
fn taken_port_is_named_with_a_free_set() {
    // Ports well away from other tests' defaults.
    let base = BuiltInPorts { control: 18300, broadcast: 18100, ui: 20000, rigctld: 14532 };
    let hold = TcpListener::bind(("127.0.0.1", base.broadcast)).unwrap();
    let PortTaken { port, suggestion } = check_free(&base).unwrap_err();
    assert_eq!(port, base.broadcast);
    assert_ne!(suggestion, base);
    for (_, p) in suggestion.all() {
        assert!(TcpListener::bind(("127.0.0.1", p)).is_ok(), "suggested port {p} is free");
    }
    drop(hold);
    assert!(check_free(&base).is_ok());
}
