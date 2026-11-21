//! The Mercury contract suite (M3c design Part A): every Mercury behaviour
//! MercuryChat relies on, written once and run against the fake
//! (`MC_CONTRACT_TARGET` unset or `fake`, in `scripts/check.sh`) and against
//! real Mercury in the Docker lab (`MC_CONTRACT_TARGET=real`, through
//! `lab/docker/contract/check-mercury.sh`).

mod attach;
mod data;
mod session;
