#!/usr/bin/env bash
# Build the M5 built-in lab program (crates/mc-core/examples/builtin_lab.rs)
# for linux-amd64 inside mercurychat-app-build:amd64, as build-rig-lab.sh does.
# Source is mounted read-only at /src; build state lives in /build (a Docker
# volume); the binary goes to /out/builtin_lab. Crates come from the host's
# cache mounted at /hostreg (read-only): no downloads.
set -euo pipefail
rm -rf /build/src-builtin && mkdir -p /build/src-builtin
tar -C /src --exclude=./target --exclude=./node_modules --exclude=./apps/desktop/node_modules \
    --exclude=./lab --exclude=./evidence --exclude=./.superpowers -cf - . | tar -C /build/src-builtin -xf -
cd /build/src-builtin
export CARGO_TARGET_DIR=/build/target
export CARGO_HOME=/build/cargo-home-m5
mkdir -p "$CARGO_HOME/registry"
# Refreshed every run (-u copies only what is newer): a copy taken once goes stale when a crate is added.
cp -ru /hostreg/cache /hostreg/index "$CARGO_HOME/registry/"
cargo build --offline --release -p mc-core --example builtin_lab
mkdir -p /out
cp "$CARGO_TARGET_DIR/release/examples/builtin_lab" /out/builtin_lab
{
  echo "rustc=$(rustc --version)"
  echo "source_commit=$(git -C /src rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "sha256_builtin_lab=$(sha256sum /out/builtin_lab | cut -d' ' -f1)"
  echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > /out/MANIFEST.txt
cat /out/MANIFEST.txt
