#!/usr/bin/env bash
# Stop or start Mercury A inside a running KEEP=1 lab (run-lab.sh), with the
# same audio route, ports and PTT none as at lab start:
#   docker exec mc-lab bash /scripts/mercury-a.sh stop|start
set -euo pipefail
BIN=${BIN:-/mercury/linux-amd64/mercury}
OUT=${OUT:-/out}
export XDG_RUNTIME_DIR=/tmp/pa-runtime
case "${1:-}" in
  stop)
    kill "$(cat "$OUT/a/pid")" 2>/dev/null || true
    for i in $(seq 1 50); do kill -0 "$(cat "$OUT/a/pid")" 2>/dev/null || break; sleep 0.1; done
    echo "mercury_a_stopped_utc=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)" >> "$OUT/lab-manifest.txt" ;;
  start)
    nohup "$BIN" -C "$OUT/a/mercury.ini" -x pulse -o a_to_b -i b_to_a.monitor -p 8300 -b 8100 -P none \
      -L "$OUT/a/mercury.log" >> "$OUT/a/stdout.log" 2>&1 &
    echo $! > "$OUT/a/pid"
    for i in $(seq 1 100); do timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/8300" 2>/dev/null && break; sleep 0.1; done
    echo "mercury_a_started_utc=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)" >> "$OUT/lab-manifest.txt" ;;
  *) echo "usage: mercury-a.sh stop|start" >&2; exit 2 ;;
esac
