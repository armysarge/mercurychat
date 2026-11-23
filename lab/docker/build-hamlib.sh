#!/usr/bin/env bash
# Build Hamlib from a release tarball for one bundle target (M5 Task 1).
#   build-hamlib.sh <tarball> <linux-amd64|windows-x64> <install prefix>
# Linux: static libhamlib, so mercury and rigctld carry it (no system Hamlib).
# Windows: libhamlib-4.dll (mingw), as Mercury's own Windows build expects.
# No readline, libusb, language bindings or XML: rigctld and the library only.
set -euo pipefail
tarball=${1:?tarball}; target=${2:?target}; prefix=${3:?prefix}
src=/tmp/hamlib-$target
rm -rf "$src" "$prefix" && mkdir -p "$src"
tar -xzf "$tarball" -C "$src" --strip-components=1
cd "$src"
common=(--prefix="$prefix" --without-readline --without-libusb --without-cxx-binding
        --without-perl-binding --without-python-binding --without-tcl-binding
        --without-lua-binding --without-xml-support --disable-html-matrix)
case "$target" in
  linux-amd64) ./configure "${common[@]}" --disable-shared --enable-static CFLAGS="-O2 -fPIC" ;;
  windows-x64) ./configure "${common[@]}" --host=x86_64-w64-mingw32 --enable-shared --disable-static ;;
  *) echo "unknown target $target" >&2; exit 2 ;;
esac > "/tmp/hamlib-configure-$target.log" 2>&1 || { tail -30 "/tmp/hamlib-configure-$target.log" >&2; exit 1; }
make -j"$(nproc)" > "/tmp/hamlib-make-$target.log" 2>&1 || { tail -40 "/tmp/hamlib-make-$target.log" >&2; exit 1; }
make install > /dev/null
echo "hamlib $target installed in $prefix"
