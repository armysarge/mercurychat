//! Hamlib rigctld: the protocol, the client task and (under `fake`) a fake
//! rigctld for tests and the simulator (M3b).

mod proto;

pub use proto::{block_complete, parse_reply, Level, RigCaps, RigProtoError, RigReply, RigRequest};
