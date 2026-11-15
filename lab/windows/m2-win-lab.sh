#!/usr/bin/env bash
# M2 Task 10 step 2: the same-OS control for the cross-OS round trip. Two
# native mercury.exe over the crossed VB-Audio cables (as run-win-lab.sh),
# the headless peer (`m0-client.exe peer --echo`) on Mercury B, and the
# release MercuryChat on Mercury A, driven by m2-ui-check.mjs --existing.
#
#   bash lab/windows/m2-win-lab.sh <run name>          (from the repo root, Git Bash)
#   bash lab/windows/m2-win-lab.sh --a stop|start DIR  (used by the check itself)
set -euo pipefail

M=${M:-lab/bin/windows-x64/mercury.exe}
CA_IN=${CA_IN:-'{0.0.0.00000000}.{1f68aa44-9ff8-4523-8429-02de487b53e0}'}   # CABLE-A Input
CA_OUT=${CA_OUT:-'{0.0.1.00000000}.{a3544a95-51f9-4cdc-aee9-359bd5b81053}'} # CABLE-A Output
CB_IN=${CB_IN:-'{0.0.0.00000000}.{964afa2f-cfaa-4fca-90ad-66241370063e}'}   # CABLE-B Input
CB_OUT=${CB_OUT:-'{0.0.1.00000000}.{22742773-d4c6-4f25-bdd3-686ec5e365cb}'} # CABLE-B Output

start_a() {
  "$M" -C "$1/a/mercury.ini" -x wasapi -o "$CA_IN" -i "$CB_OUT" -p 8300 -b 8100 -P none -L "$1/a/mercury.log" >> "$1/a/stdout.log" 2>&1 &
  echo $! > "$1/a/pid"
  for i in $(seq 1 100); do (exec 3<>"/dev/tcp/127.0.0.1/8300") 2>/dev/null && break; sleep 0.1; done
  echo "mercury_a_started_utc=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)" >> "$1/lab-manifest.txt"
}

if [ "${1:-}" = "--a" ]; then
  case "$2" in
    stop)
      kill "$(cat "$3/a/pid")" 2>/dev/null || true
      for i in $(seq 1 50); do kill -0 "$(cat "$3/a/pid")" 2>/dev/null || break; sleep 0.1; done
      echo "mercury_a_stopped_utc=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)" >> "$3/lab-manifest.txt" ;;
    start) start_a "$3" ;;
  esac
  exit 0
fi

RUN=${1:?usage: m2-win-lab.sh <run name>}
OUT=lab/run/$RUN
[ -e "$OUT" ] && { echo "$OUT exists: use a fresh run name" >&2; exit 2; }
mkdir -p "$OUT/a" "$OUT/b" "$OUT/peer"
APP=${APP:-target/release/mercurychat.exe}
PEER_BIN=${PEER_BIN:-target/release/m0-client.exe}
for s in a b; do printf '[store]\nenabled = false\n' > "$OUT/$s/mercury.ini"; done
{
  echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mercury_sha256=$(sha256sum "$M" | cut -d' ' -f1)"
  echo "app_sha256=$(sha256sum "$APP" | cut -d' ' -f1)"
  echo "peer_sha256=$(sha256sum "$PEER_BIN" | cut -d' ' -f1)"
  echo "audio=wasapi; A out CABLE-A Input / in CABLE-B Output; B out CABLE-B Input / in CABLE-A Output; PTT none"
} > "$OUT/lab-manifest.txt"

start_a "$OUT"
"$M" -C "$OUT/b/mercury.ini" -x wasapi -o "$CB_IN" -i "$CA_OUT" -p 8400 -b 8200 -P none -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
PB=$!
for i in $(seq 1 100); do (exec 3<>"/dev/tcp/127.0.0.1/8400") 2>/dev/null && break; sleep 0.1; done
"$PEER_BIN" peer --host 127.0.0.1 --port 8400 --call "${PEER_CALL:-SIM2}" --echo --work "$OUT/peer/station" \
  > "$OUT/peer/peer.jsonl" 2> "$OUT/peer/peer.log" &
PP=$!
trap 'kill $PP $PB "$(cat "$OUT/a/pid")" 2>/dev/null || true' EXIT
for i in $(seq 1 100); do grep -q '"listening"' "$OUT/peer/peer.jsonl" 2>/dev/null && break; sleep 0.1; done
grep -q '"listening"' "$OUT/peer/peer.jsonl" || { echo "the peer did not start" >&2; cat "$OUT/peer/peer.log" >&2; exit 1; }

DATA=$(mktemp -d)
# The check runs these through cmd.exe: name this Git Bash, never WSL's bash.
SH=$(cygpath -w "$BASH")
node lab/windows/m2-ui-check.mjs "$(cygpath -w "$PWD/$APP")" "$(cygpath -w "$DATA")" 9336 \
  --existing 127.0.0.1:8300 --peer "${PEER_CALL:-SIM2}" --peer-log "$OUT/peer/peer.jsonl" \
  --stop-a "\"$SH\" lab/windows/m2-win-lab.sh --a stop $OUT" \
  --start-a "\"$SH\" lab/windows/m2-win-lab.sh --a start $OUT" | tee "$OUT/m2-ui-check.txt"
