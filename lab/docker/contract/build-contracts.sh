#!/usr/bin/env bash
# Build the Mercury contract test binary for linux-amd64 inside
# mercurychat-app-build:amd64. Source is mounted read-only at /src; build
# state lives in /build (a Docker volume); the binary goes to /out/contracts.
set -euo pipefail
rm -rf /build/src-contract && mkdir -p /build/src-contract
tar -C /src --exclude=./target --exclude=./node_modules --exclude=./apps/desktop/node_modules \
    --exclude=./lab --exclude=./evidence --exclude=./.superpowers -cf - . | tar -C /build/src-contract -xf -
cd /build/src-contract
export CARGO_TARGET_DIR=/build/target
cargo test -p mercury-contract --no-run --message-format=json > /tmp/build.json
exe=$(grep '"reason":"compiler-artifact"' /tmp/build.json | grep '"name":"contracts"' \
      | grep -o '"executable":"[^"]*"' | cut -d'"' -f4 | tail -1)
[ -n "$exe" ] && [ -x "$exe" ] || { echo "no contracts test executable in the build output" >&2; exit 1; }
mkdir -p /out
cp "$exe" /out/contracts
{
  echo "rustc=$(rustc --version)"
  echo "source_commit=$(git -C /src rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "sha256_contracts=$(sha256sum /out/contracts | cut -d' ' -f1)"
  echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > /out/MANIFEST.txt
cat /out/MANIFEST.txt
