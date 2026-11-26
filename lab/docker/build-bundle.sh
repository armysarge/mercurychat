#!/usr/bin/env bash
# Build MercuryChat's built-in modem bundle (M5 Task 1) inside mercurychat-bundle:jammy:
# Hamlib, then Mercury (+ lab/mercury-patches) linked against it, then rigctld,
# the libraries, BUILD.txt and MANIFEST.sha256, then check-bundle.sh.
#
#   build-bundle.sh [--latest] [linux-amd64] [windows-x64]
#
# Default (development, labs): the fixed pins in lab/hamlib-pin.env and
# lab/mercury-pin.env, from the read-only fork mount and a local tarball.
# --latest (release builds, design §3): the newest Hamlib and Mercury releases,
# exactly as lab/release/resolve-release.sh resolved them and had them approved:
# the Hamlib tarball must have that SHA-256, and Mercury is that commit, cloned
# inside this container. A patch that doesn't apply or a failed check stops the build.
#
# Mounts: /scripts (lab/docker, ro), /pins (lab, ro), /patches (lab/mercury-patches, ro),
#         /src (the Mercury fork, ro), /hamlib-src (dir with the pinned tarball, ro), /out,
#         with --latest /resolved (resolve-release.sh's output, ro; or $MC_RESOLVED)
set -euo pipefail
latest=0
if [ "${1:-}" = --latest ]; then latest=1; shift; fi
targets=("$@"); [ ${#targets[@]} -eq 0 ] && targets=(linux-amd64 windows-x64)

# shellcheck disable=SC1091
. /pins/hamlib-pin.env
# shellcheck disable=SC1091
. /pins/mercury-pin.env
mercury_repo=/src; mercury_ref=$MERCURY_COMMIT
if [ $latest = 1 ]; then
  resolved=${MC_RESOLVED:-/resolved}
  [ -f "$resolved/resolved.env" ] \
    || { echo "FAIL no approved sources in $resolved: run lab/release/resolve-release.sh first" >&2; exit 4; }
  # Nothing from the pins may stand in for a value the resolver did not give.
  unset HAMLIB_VERSION HAMLIB_SHA256 MERCURY_REPO MERCURY_REF
  # shellcheck disable=SC1091
  . "$resolved/resolved.env"
  : "${HAMLIB_VERSION:?}" "${HAMLIB_SHA256:?}" "${MERCURY_REPO:?}" "${MERCURY_REF:?}"
  [[ $MERCURY_REF =~ ^[0-9a-f]{40}$ ]] || { echo "FAIL Mercury's ref '$MERCURY_REF' is not a full commit" >&2; exit 4; }
  mercury_repo=$MERCURY_REPO; mercury_ref=$MERCURY_REF
  tarball=$resolved/hamlib-$HAMLIB_VERSION.tar.gz
  expected="the approved one ($resolved/resolved.env)"
else
  tarball=/hamlib-src/hamlib-$HAMLIB_VERSION.tar.gz
  expected="lab/hamlib-pin.env"
fi
echo "$HAMLIB_SHA256  $tarball" | sha256sum -c --quiet \
  || { echo "FAIL the Hamlib tarball's SHA-256 differs from $expected" >&2; exit 1; }

rm -rf /m && git clone -q "$mercury_repo" /m && git -C /m checkout -q "$mercury_ref"
commit=$(git -C /m rev-parse HEAD)
[ $latest = 0 ] || [ "$commit" = "$mercury_ref" ] || { echo "FAIL checked out $commit, not $mercury_ref" >&2; exit 1; }
mversion=$(git -C /m describe --tags --always 2>/dev/null || echo "$commit")
applied=""
for p in $MERCURY_PATCHES; do
  if ! git -C /m apply --check --whitespace=nowarn "/patches/$p" 2>/dev/null; then
    echo "FAIL patch ${p%%-*} does not apply to Mercury $mversion" >&2
    exit 3
  fi
  git -C /m apply --whitespace=nowarn "/patches/$p"
  applied="$applied $p@$(sha256sum "/patches/$p" | cut -c1-12)"
done

# The exact source built (M5 Task 12, GPL): Mercury as patched, the patches,
# and the Hamlib tarball, beside the programs in /out/source.
rm -rf /out/source && mkdir -p /out/source
tar -C / --exclude=m/.git -czf "/out/source/mercury-$mversion-source.tar.gz" m
for p in $MERCURY_PATCHES; do cp "/patches/$p" /out/source/; done
cp "$tarball" /out/source/
( cd /out/source && sha256sum ./* > SHA256SUMS )

for t in "${targets[@]}"; do
  echo "=== $t ==="
  prefix=/opt/hamlib/$t
  bash /scripts/build-hamlib.sh "$tarball" "$t" "$prefix"
  cd /m && make clean > /dev/null 2>&1 || true
  out=/out/$t
  rm -rf "$out" && mkdir -p "$out"
  case "$t" in
    linux-amd64)
      # Mercury's Makefiles find Hamlib with pkg-config: point it at ours, and
      # link the static library with its private libraries.
      export PKG_CONFIG_PATH=$prefix/lib/pkgconfig
      hl=$(pkg-config --static --libs hamlib)
      make -j"$(nproc)" HAVE_HAMLIB=1 HAMLIB_CFLAGS="-I$prefix/include -DHAVE_HAMLIB" \
        HAMLIB_CFLAGS_LOCAL="-I$prefix/include -DHAVE_HAMLIB" \
        HAMLIB_LDFLAGS="$hl" > /tmp/mercury-$t.log 2>&1 || { grep -m5 -E "error|Error" /tmp/mercury-$t.log >&2; exit 1; }
      unset PKG_CONFIG_PATH
      cp mercury "$out/"
      cp "$prefix/bin/rigctld" "$out/" ;;
    windows-x64)
      # radio_io's own Makefile uses the vendored hamlib-w64 headers; ours must
      # match the DLL we ship, so both include paths point at our Hamlib.
      make windows HAMLIB_W64_DIR="$prefix" HAMLIB_CFLAGS_LOCAL="-I$prefix/include -DHAVE_HAMLIB" \
        > /tmp/mercury-$t.log 2>&1 || { grep -m5 -E "error|Error" /tmp/mercury-$t.log >&2; exit 1; }
      cp "$(ls mercury.exe 2>/dev/null || find . -maxdepth 2 -name mercury.exe | head -1)" "$out/"
      cp "$prefix/bin/rigctld.exe" "$prefix/bin/libhamlib-4.dll" "$out/"
      # The mingw runtime DLLs the programs import, from the toolchain.
      for round in 1 2; do
        for f in "$out"/*.exe "$out"/*.dll; do
          for d in $(x86_64-w64-mingw32-objdump -p "$f" | awk '/DLL Name:/ {print $3}'); do
            [ -f "$out/$d" ] && continue
            src=$(find /usr/x86_64-w64-mingw32 /usr/lib/gcc/x86_64-w64-mingw32 -name "$d" 2>/dev/null | head -1)
            [ -n "$src" ] && cp "$src" "$out/"
          done
        done
      done ;;
    *) echo "unknown target $t" >&2; exit 2 ;;
  esac
  # Ship without debug symbols (the DLL is 30 MB with them).
  case "$t" in
    linux-amd64) strip --strip-unneeded "$out/mercury" "$out/rigctld" ;;
    windows-x64) x86_64-w64-mingw32-strip --strip-unneeded "$out"/*.exe "$out"/*.dll ;;
  esac
  {
    echo "target=$t"
    echo "hamlib_version=$HAMLIB_VERSION"
    echo "hamlib_sha256=$HAMLIB_SHA256"
    echo "mercury_version=$mversion"
    echo "mercury_commit=$commit"
    echo "mercury_patches=${applied:- none}"
    echo "mode=$([ $latest = 1 ] && echo latest || echo pinned)"
    echo "image=$(. /etc/os-release && echo "$PRETTY_NAME")"
    echo "built_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "$out/BUILD.txt"
  ( cd "$out" && find . -type f ! -name MANIFEST.sha256 | sed 's|^\./||' | sort | xargs sha256sum > MANIFEST.sha256 )
  bash /scripts/check-bundle.sh "$out" "$t" "$HAMLIB_VERSION"
done
