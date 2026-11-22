#!/usr/bin/env bash
# Run the Mercury contract suite against real Mercury (M3c design A1-A5), from
# the Windows host in Git Bash. Three Docker steps:
#
#   1. build Mercury from the fork (read-only) with build-mercury.sh;
#   2. build the contract test binary with build-contracts.sh;
#   3. run it against two Mercury stations with run-contracts.sh.
#
# Usage: lab/docker/contract/check-mercury.sh [--latest] [--commit <sha>] [--out <dir>]
#
#   (default)        the pin in lab/mercury-pin.env, with its patches
#   --latest         the fork's upstream/mercuryv2, no patches (no network fetch)
#   --commit <sha>   that commit, no patches
#   --out <dir>      default E:/mc-lab-out/m4/contracts-<sha7>[-latest]
#
# The summary is <out>/run/summary.txt; the exit code is non-zero on any FAIL.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
win() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else echo "$1"; fi; }
FORK=${MERCURY_FORK:-E:/Programming/mercury-fork}

# shellcheck source=../../mercury-pin.env
. "$repo/lab/mercury-pin.env"
commit=$MERCURY_COMMIT
patches=$MERCURY_PATCHES
suffix=""
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --latest) commit=$(git -C "$FORK" rev-parse 'upstream/mercuryv2^{commit}'); patches=""; suffix=-latest ;;
    --commit) commit=$(git -C "$FORK" rev-parse "$2^{commit}"); patches=""; shift ;;
    --out) out=$2; shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done
out=${out:-E:/mc-lab-out/m4/contracts-${commit:0:7}$suffix}
case "$(win "$out")" in [Ff]:*) echo "refusing to write to F: ($out)" >&2; exit 2 ;; esac
mkdir -p "$out/bin" "$out/contracts" "$out/run" "$out/patches"
rm -f "$out/patches"/*.patch
for p in $patches; do cp "$repo/lab/mercury-patches/$p" "$out/patches/"; done
echo "commit=$commit patches=${patches:-none} out=$out"

export MSYS_NO_PATHCONV=1
R=$(win "$repo"); O=$(win "$out")

echo "=== 1. Mercury $commit"
docker run --rm -e MERCURY_COMMIT="$commit" -v "$FORK:/src:ro" -v "$O/bin:/out" \
  -v "$R/lab/docker:/scripts:ro" -v "$O/patches:/patches:ro" mercurychat-lab:m0 \
  bash -c 'git config --global --add safe.directory "*" && bash /scripts/build-mercury.sh linux-amd64' \
  > "$out/build-mercury.log" 2>&1 || { tail -20 "$out/build-mercury.log"; exit 1; }

echo "=== 1b. listen-mode frame sizes against tests/fixtures/mercury-listen-modes.txt"
docker run --rm -v "$O/bin:/mercury:ro" mercurychat-lab:rig bash -c '/mercury/linux-amd64/mercury -l 2>&1' \
  | awk '/^Mode index/{i=$3} /^Opening mode/{n=$3} /payload_bytes_per_modem_frame/{print i, n, $2}' > "$out/listen-modes.txt"
if ! diff <(grep -v '^#' "$repo/tests/fixtures/mercury-listen-modes.txt") "$out/listen-modes.txt"; then
  echo "listen-mode frame sizes differ from the fixture: update kiss.rs and beacon.rs (and the fixture) for this build" >&2
  [ -z "$patches" ] || exit 1   # the pin must match; --latest / --commit only report it
fi

echo "=== 2. the contract binary"
docker run --rm -v "$R:/src:ro" -v mc-app-build-amd64:/build -v "$O/contracts:/out" \
  -v "$R/lab/docker/contract:/scripts:ro" mercurychat-app-build:amd64 bash /scripts/build-contracts.sh \
  > "$out/build-contracts.log" 2>&1 || { tail -20 "$out/build-contracts.log"; exit 1; }

echo "=== 3. the contracts against real Mercury"
set +e
docker run --rm -v "$O/bin:/mercury:ro" -v "$O/contracts:/contracts:ro" -v "$O/run:/out" \
  -v "$R/lab/docker/contract:/scripts:ro" mercurychat-lab:rig bash /scripts/run-contracts.sh \
  > "$out/run-contracts.log" 2>&1
rc=$?
set -e
cat "$out/run/summary.txt" 2>/dev/null || tail -30 "$out/run-contracts.log"
exit $rc
