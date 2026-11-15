#!/usr/bin/env bash
# L1 virtual-audio lab (docs/07 §2): two Mercury instances, two independent
# crossed PulseAudio null-sink routes, PTT none, separate configs/ports/state.
#
#   Station A: plays into a_to_b, captures b_to_a.monitor   control 8300/8301, broadcast 8100
#   Station B: plays into b_to_a, captures a_to_b.monitor   control 8400/8401, broadcast 8200
#
# Neither station hears its own playback. Runs inside mercurychat-lab:m0 with
# /mercury (binaries, read-only) and /out (logs, manifest) mounted.
set -euo pipefail

BIN=${BIN:-/mercury/linux-amd64/mercury}
OUT=${OUT:-/out}
RATE=48000
mkdir -p "$OUT/a" "$OUT/b"

# --- PulseAudio, private to this container, never touching host devices ----
export XDG_RUNTIME_DIR=/tmp/pa-runtime; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
pulseaudio --daemonize=yes --exit-idle-time=-1 --disallow-exit=yes --system=no \
  -n --load=module-native-protocol-unix --load="module-always-sink" \
  --log-target=file:"$OUT/pulseaudio.log" 2>/dev/null || true
for i in $(seq 1 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
pactl info >/dev/null
pactl load-module module-null-sink sink_name=a_to_b rate=$RATE channels=2 \
  sink_properties=device.description=LabRouteAtoB >/dev/null
pactl load-module module-null-sink sink_name=b_to_a rate=$RATE channels=2 \
  sink_properties=device.description=LabRouteBtoA >/dev/null
# No default device may be used as a fallback: point defaults at the
# always-sink placeholder, not at a lab route.
pactl set-default-sink auto_null 2>/dev/null || true

# --- per-station config ---------------------------------------------------
write_ini() { # name base bcast out_dev in_dev
  cat > "$OUT/$1/mercury.ini" <<EOF
[main]
ui_enabled = false
sound_system = pulse
output_device = $4
input_device = $5
capture_channel = left
arq_tcp_base_port = $2
broadcast_tcp_port = $3
log_utc = true
verbose = false
[ptt]
method = none
[store]
enabled = false
EOF
}
write_ini a 8300 8100 a_to_b b_to_a.monitor
write_ini b 8400 8200 b_to_a a_to_b.monitor

{
  echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mercury_bin=$BIN"
  echo "mercury_sha256=$(sha256sum "$BIN" | cut -d' ' -f1)"
  echo "mercury_version=$("$BIN" -V 2>&1 | tr -d '\033' | sed 's/\[[0-9;]*m//g' | grep -i version | head -1)"
  echo "pulseaudio=$(pulseaudio --version)"
  echo "route_a_to_b=$(pactl list short sinks | grep a_to_b)"
  echo "route_b_to_a=$(pactl list short sinks | grep b_to_a)"
  echo "kernel=$(uname -srm)"
} > "$OUT/lab-manifest.txt"

# --- route isolation check: A->B carries signal, B's own route stays silent --
check_route() { # sink monitor_should_hear monitor_should_not_hear
  local hear quiet
  ( timeout -s INT 2 parec -d "$2" --format=s16le --rate=$RATE --channels=1 --raw --latency-msec=20 > /tmp/hear.raw ) &
  ( timeout -s INT 2 parec -d "$3" --format=s16le --rate=$RATE --channels=1 --raw --latency-msec=20 > /tmp/quiet.raw ) &
  sleep 0.3
  # 0.8 s of a 1 kHz-ish square tone into the sink under test.
  python3 - "$1" <<'PY' 2>/dev/null || head -c 76800 /dev/urandom | pacat --playback -d "$1" --format=s16le --rate=48000 --channels=1 --raw
import sys, struct, subprocess
pcm = b"".join(struct.pack("<h", 8000 if (i // 24) % 2 else -8000) for i in range(38400))
subprocess.run(["pacat", "--playback", "-d", sys.argv[1], "--format=s16le", "--rate=48000", "--channels=1", "--raw"], input=pcm)
PY
  wait
  hear=$(od -An -td2 -w2 -v /tmp/hear.raw | awk '{s=($1<0?-$1:$1); if (s>m) m=s} END {print m+0}')
  quiet=$(od -An -td2 -w2 -v /tmp/quiet.raw | awk '{s=($1<0?-$1:$1); if (s>m) m=s} END {print m+0}')
  echo "route $1: peak on $2 = $hear, peak on $3 = $quiet" | tee -a "$OUT/lab-manifest.txt"
  [ "$hear" -gt 1000 ] && [ "$quiet" -lt 100 ]
}
check_route a_to_b a_to_b.monitor b_to_a.monitor
check_route b_to_a b_to_a.monitor a_to_b.monitor
echo "route_isolation=PASS" >> "$OUT/lab-manifest.txt"

# --- start both modems (PTT none, private configs) ----------------------------
"$BIN" -C "$OUT/a/mercury.ini" -x pulse -o a_to_b -i b_to_a.monitor -p 8300 -b 8100 -P none -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 &
echo $! > "$OUT/a/pid"
"$BIN" -C "$OUT/b/mercury.ini" -x pulse -o b_to_a -i a_to_b.monitor -p 8400 -b 8200 -P none -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
echo $! > "$OUT/b/pid"

for port in 8300 8301 8400 8401; do
  # Connect-only probe. Never write here: Mercury queues bytes written to an
  # idle data port and transmits them at the start of the next session.
  for i in $(seq 1 100); do timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$port" 2>/dev/null && break; sleep 0.1; done
done
# --- optional headless MercuryChat peer on Mercury B (M2) ---------------------
# PEER=1 runs `m0-client peer --echo` on station B, so a MercuryChat on A can
# call it. One JSON line per stored message goes to $OUT/peer/peer.jsonl.
if [ "${PEER:-0}" = 1 ]; then
  PEER_BIN=${PEER_BIN:-/mercury/linux-amd64/m0-client}
  [ -x "$PEER_BIN" ] || { echo "PEER=1 but no m0-client at $PEER_BIN" >&2; exit 1; }
  mkdir -p "$OUT/peer"
  "$PEER_BIN" peer --host 127.0.0.1 --port 8400 --call "${PEER_CALL:-SIM2}" --echo --work "$OUT/peer/station" \
    > "$OUT/peer/peer.jsonl" 2> "$OUT/peer/peer.log" &
  echo $! > "$OUT/peer/pid"
  for i in $(seq 1 100); do grep -q '"listening"' "$OUT/peer/peer.jsonl" 2>/dev/null && break; sleep 0.1; done
  grep -q '"listening"' "$OUT/peer/peer.jsonl" || { echo "the peer did not start; see $OUT/peer/peer.log" >&2; exit 1; }
  echo "peer=${PEER_CALL:-SIM2} on B (echo), sha256=$(sha256sum "$PEER_BIN" | cut -d' ' -f1)" >> "$OUT/lab-manifest.txt"
fi

echo "LAB READY: A=8300/8301 B=8400/8401 (PTT none, crossed null-sink routes)"
echo "lab_ready_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$OUT/lab-manifest.txt"

# Sample modem CPU/RSS every 5 s while the lab runs.
( while sleep 5; do
    for s in a b; do
      pid=$(cat "$OUT/$s/pid"); ps -o pid=,pcpu=,rss= -p "$pid" 2>/dev/null | awk -v s=$s -v t="$(date -u +%H:%M:%S)" '{print t, s, "cpu%=" $2, "rss_kb=" $3}'
    done
  done ) >> "$OUT/resources.log" &

wait -n "$(cat "$OUT/a/pid")" "$(cat "$OUT/b/pid")" || true
echo "a Mercury process exited" | tee -a "$OUT/lab-manifest.txt"
