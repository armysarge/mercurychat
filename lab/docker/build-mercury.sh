#!/usr/bin/env bash
# Build Mercury from the read-only source mount (/src) for the M0 targets and write
# binaries plus a provenance manifest to /out/<target>/.
#
# Usage (inside the lab image):  build-mercury.sh [linux-amd64] [linux-arm64] [windows-x64]
set -euo pipefail

targets=("$@")
[ ${#targets[@]} -eq 0 ] && targets=(linux-amd64 linux-arm64 windows-x64)

commit=$(git -C /src rev-parse HEAD 2>/dev/null || echo unknown)
dirty=$(git -C /src status --porcelain --untracked-files=no 2>/dev/null | wc -l || echo unknown)

for t in "${targets[@]}"; do
  echo "=== building $t ==="
  # Clone (not copy): the Windows checkout has CRLF endings that break the build
  # scripts, and a clone pins exactly the recorded commit.
  cd / && rm -rf /build && git clone -q /src /build && git -C /build checkout -q "$commit" && cd /build
  make clean >/dev/null 2>&1 || true
  case "$t" in
    linux-amd64)
      make -j"$(nproc)"
      bin=mercury; cc=gcc ;;
    linux-arm64)
      # Generic ARMv8-A baseline (no -mcpu=native auto-detect on the build host):
      # see docs/07 §6 — a Pi 5 tuned build must not pose as the Pi 4 baseline.
      export PKG_CONFIG_ALLOW_CROSS=1
      export PKG_CONFIG_LIBDIR=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig:/usr/lib/pkgconfig
      make -j"$(nproc)" CC=aarch64-linux-gnu-gcc
      unset PKG_CONFIG_ALLOW_CROSS PKG_CONFIG_LIBDIR
      bin=mercury; cc=aarch64-linux-gnu-gcc ;;
    windows-x64)
      make windows
      bin=$(ls mercury.exe 2>/dev/null || find . -maxdepth 2 -name 'mercury.exe' | head -1)
      cc=x86_64-w64-mingw32-gcc ;;
    *) echo "unknown target $t" >&2; exit 2 ;;
  esac
  mkdir -p "/out/$t"
  cp "$bin" "/out/$t/"
  # mercury.exe imports libhamlib-4.dll; ship the vendored DLLs beside it, as
  # Mercury's own windows-zip target does (Makefile: HAMLIB_W64_DIR/bin/*.dll).
  if [ "$t" = windows-x64 ]; then
    cp radio_io/hamlib-w64/bin/*.dll "/out/$t/"
    ( cd "/out/$t" && sha256sum *.dll > DLL-SHA256.txt )
  fi
  out_bin="/out/$t/$(basename "$bin")"
  {
    echo "target=$t"
    echo "mercury_commit=$commit"
    echo "tracked_changes=$dirty"
    echo "compiler=$($cc --version | head -1)"
    echo "file=$(file -b "$out_bin")"
    echo "sha256=$(sha256sum "$out_bin" | cut -d' ' -f1)"
    echo "size=$(stat -c %s "$out_bin")"
    echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "image=debian:trixie"
    echo "arm64_cpu_baseline=$( [ "$t" = linux-arm64 ] && echo "generic armv8-a + -moutline-atomics (no PLATFORM, no devicetree in build container)" || echo n/a)"
  } > "/out/$t/MANIFEST.txt"
  cat "/out/$t/MANIFEST.txt"
done
