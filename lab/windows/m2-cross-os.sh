#!/usr/bin/env bash
# M2 Task 10: cross-OS chat round trip over real Mercury (no RF, PTT none).
# The Linux docker lab runs Mercury A and B over crossed PulseAudio null sinks,
# with the headless peer (`m0-client peer --echo`, Linux build) on B. The
# Windows release build of MercuryChat attaches to Mercury A on
# 127.0.0.1:8300 and is driven by m2-ui-check.mjs --existing.
#
#   bash lab/windows/m2-cross-os.sh <run name>
# Needs: lab/bin/linux-amd64/mercury, lab/run/m2-peer-bin/m0-client (Linux amd64),
# target/release/mercurychat.exe, image mercurychat-lab:m0.
set -euo pipefail

RUN=${1:?usage: m2-cross-os.sh <run name>}
OUT=lab/run/$RUN
[ -e "$OUT" ] && { echo "$OUT exists: use a fresh run name" >&2; exit 2; }
mkdir -p "$OUT"
NAME=mc-lab-m2
APP=${APP:-target/release/mercurychat.exe}
PEER_DIR=${PEER_DIR:-lab/run/m2-peer-bin}

docker rm -f "$NAME" >/dev/null 2>&1 || true
MSYS_NO_PATHCONV=1 docker run -d --name "$NAME" \
  -p 127.0.0.1:8300-8301:8300-8301 \
  -e PEER=1 -e KEEP=1 -e PEER_BIN=/peer/m0-client \
  -v "$PWD/lab/bin:/mercury:ro" -v "$PWD/$PEER_DIR:/peer:ro" \
  -v "$PWD/$OUT:/out" -v "$PWD/lab/docker:/scripts:ro" \
  mercurychat-lab:m0 bash /scripts/run-lab.sh >/dev/null
trap 'docker logs "$NAME" > "$OUT/container.log" 2>&1 || true; docker rm -f "$NAME" >/dev/null 2>&1 || true' EXIT

for i in $(seq 1 120); do docker logs "$NAME" 2>&1 | grep -q "LAB READY" && break; sleep 1; done
docker logs "$NAME" 2>&1 | grep -q "LAB READY" || { echo "the lab did not start" >&2; docker logs "$NAME" >&2; exit 1; }

{
  echo "app_sha256=$(sha256sum "$APP" | cut -d' ' -f1)"
  echo "host=$(uname -s) $(uname -r)"
} >> "$OUT/lab-manifest.txt"

DATA=$(mktemp -d)
node lab/windows/m2-ui-check.mjs "$(cygpath -w "$PWD/$APP")" "$(cygpath -w "$DATA")" 9335 \
  --existing 127.0.0.1:8300 --peer "${PEER_CALL:-SIM2}" --peer-log "$OUT/peer/peer.jsonl" \
  --stop-a "docker exec $NAME bash /scripts/mercury-a.sh stop" \
  --start-a "docker exec $NAME bash /scripts/mercury-a.sh start" | tee "$OUT/m2-ui-check.txt"
