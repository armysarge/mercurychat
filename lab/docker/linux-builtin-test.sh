#!/usr/bin/env bash
# Run mc-core built-in modem tests on Linux (pdeathsig, SIGTERM paths), in
# mercurychat-app-build:amd64 with volume mc-app-build-amd64, the worktree at
# /src (ro), this folder at /s (ro) and the host crate cache at /hostreg (ro).
set -euo pipefail
rm -rf /build/src-m5 && mkdir -p /build/src-m5
tar -C /src --exclude=./target --exclude=./node_modules --exclude=./apps/desktop/node_modules \
    --exclude=./lab --exclude=./evidence --exclude=./.superpowers -cf - . | tar -C /build/src-m5 -xf -
cd /build/src-m5
export CARGO_TARGET_DIR=/build/target
# The host's crate cache, copied in once: no downloads.
export CARGO_HOME=/build/cargo-home-m5
mkdir -p $CARGO_HOME/registry
[ -d $CARGO_HOME/registry/cache ] || cp -r /hostreg/cache /hostreg/index $CARGO_HOME/registry/
cargo test --offline -p mc-core --features sim --test builtin 2>&1 | tail -25
