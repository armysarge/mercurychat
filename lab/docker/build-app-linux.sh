#!/usr/bin/env bash
# Build MercuryChat .deb packages for linux-amd64 and linux-arm64 inside
# mercurychat-app-build. Source is mounted read-only at /src; build state lives
# in /build (a Docker volume), results go to /out/<arch>/.
set -euo pipefail
targets=("$@"); [ ${#targets[@]} -eq 0 ] && targets=(amd64 arm64)

rm -rf /build/src && mkdir -p /build/src
# Copy only what the Rust/Tauri build needs (no node_modules, no host target/).
tar -C /src --exclude=./target --exclude=./node_modules --exclude=./apps/desktop/node_modules \
    --exclude=./lab --exclude=./evidence -cf - . | tar -C /build/src -xf -
cd /build/src
test -f apps/desktop/dist/index.html || { echo "apps/desktop/dist missing: build the UI on the host first" >&2; exit 2; }
export CARGO_TARGET_DIR=/build/target
NOUI='{"build":{"beforeBuildCommand":""}}'

for a in "${targets[@]}"; do
  echo "=== app $a ==="
  case "$a" in
    amd64) cargo tauri build --bundles deb --config "$NOUI"
           dir=$CARGO_TARGET_DIR/release ;;
    arm64) export PKG_CONFIG_SYSROOT_DIR=/ PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig PKG_CONFIG_ALLOW_CROSS=1
           cargo tauri build --target aarch64-unknown-linux-gnu --bundles deb --config "$NOUI"
           unset PKG_CONFIG_SYSROOT_DIR PKG_CONFIG_PATH PKG_CONFIG_ALLOW_CROSS
           dir=$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/release ;;
  esac
  # The M0 test client for the same architecture (Pi lab runs).
  case "$a" in
    amd64) cargo build --release -p m0-client ;;
    arm64) cargo build --release -p m0-client --target aarch64-unknown-linux-gnu ;;
  esac
  mkdir -p "/out/$a"
  cp "$dir/m0-client" "/out/$a/m0-client"
  cp "$dir"/bundle/deb/*.deb "/out/$a/"
  cp "$dir/mercurychat" "/out/$a/mercurychat"
  {
    echo "arch=$a"
    echo "rustc=$(rustc --version)"
    echo "file=$(file -b "$dir/mercurychat")"
    echo "sha256_bin=$(sha256sum "$dir/mercurychat" | cut -d' ' -f1)"
    echo "sha256_m0_client=$(sha256sum "$dir/m0-client" | cut -d' ' -f1)"
    for d in "$dir"/bundle/deb/*.deb; do
      echo "deb=$(basename "$d") sha256=$(sha256sum "$d" | cut -d' ' -f1) size=$(stat -c %s "$d")"
      echo "deb_depends=$(dpkg-deb -f "$d" Depends)"
    done
    echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "/out/$a/MANIFEST.txt"
  cat "/out/$a/MANIFEST.txt"
done
