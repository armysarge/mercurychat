//! Hamlib rigctld: the protocol, the client task and (under `fake`) a fake
//! rigctld for tests and the simulator (M3b).

mod client;
mod proto;

pub use client::{
    PollRate, PttReading, RigClient, RigClientCmd, RigConn, RigIo, RigLink, RigMsg, CAPS_TIMEOUT, MAX_TIMEOUTS,
    METER_EVERY, READ_EVERY, REQUEST_TIMEOUT,
};

pub use proto::{block_complete, parse_reply, Level, RigCaps, RigProtoError, RigReply, RigRequest};
