//! Hamlib rigctld: the protocol, the client task and (under `fake`) a fake
//! rigctld for tests and the simulator (M3b).

mod client;
mod daemon;
mod probe;
mod proto;

pub use client::{
    PollRate, PttReading, RigClient, RigClientCmd, RigConn, RigIo, RigLink, RigMsg, CAPS_TIMEOUT, MAX_TIMEOUTS,
    METER_EVERY, READ_EVERY, REQUEST_TIMEOUT,
};

pub use daemon::{
    answers_as_rigctld, find_rigctld, find_rigctld_in, parse_model_list, rigctld_args, DaemonState, HamlibModel,
    RigctldDaemon, RigctldSpec, LOG_LINES, READY_WITHIN, RESTART_AFTER,
};
pub use probe::{probe, ProbeStep, RPRT_PTT_UNREADABLE};
pub use proto::{block_complete, parse_reply, Level, RigCaps, RigProtoError, RigReply, RigRequest};
