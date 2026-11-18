#!/usr/bin/env bash
# Build the M3b rig lab program (crates/mc-core/examples/rig_lab.rs) for
# linux-amd64 inside mercurychat-app-build:amd64, as build-app-linux.sh does
# for the app. Source is mounted read-only at /src; build state lives in
# /build (a Docker volume); the binary goes to /out/rig_lab.
set -euo pipefail
rm -rf /build/src-rig && mkdir -p /build/src-rig
tar -C /src --exclude=./target --exclude=./node_modules --exclude=./apps/desktop/node_modules \
    --exclude=./lab --exclude=./evidence --exclude=./.superpowers -cf - . | tar -C /build/src-rig -xf -
cd /build/src-rig
export CARGO_TARGET_DIR=/build/target
cargo build --release -p mc-core --features sim --example rig_lab
mkdir -p /out
cp "$CARGO_TARGET_DIR/release/examples/rig_lab" /out/rig_lab
{
  echo "rustc=$(rustc --version)"
  echo "sha256_rig_lab=$(sha256sum /out/rig_lab | cut -d' ' -f1)"
  echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > /out/MANIFEST.txt
cat /out/MANIFEST.txt
