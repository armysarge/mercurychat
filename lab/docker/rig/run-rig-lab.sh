#!/usr/bin/env bash
# Rig-control lab (docs/02 §5.1, decision D13). Same crossed null-sink audio as
# run-lab.sh, but each Mercury keys PTT through its own Hamlib rigctld running
# the DUMMY rig (model 1). No real transceiver, serial port or RF is involved.
#
#   Station A: rigctld 127.0.0.1:4532, Mercury -R 2 -A 127.0.0.1:4532, TNC 8300/8301, WS 10000
#   Station B: rigctld 127.0.0.1:4632, Mercury -R 2 -A 127.0.0.1:4632, TNC 8400/8401, WS 10100
#
# Then runs rig-proof.py, which plays the MercuryChat role (rigctl client +
# TNC client + WebSocket client) and writes /out/report.{json,md}.
set -euo pipefail

BIN=${BIN:-/mercury/linux-amd64/mercury}
OUT=${OUT:-/out}
RATE=48000
mkdir -p "$OUT/a" "$OUT/b"

export XDG_RUNTIME_DIR=/tmp/pa-runtime; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
pulseaudio --daemonize=yes --exit-idle-time=-1 --disallow-exit=yes --system=no \
  -n --load=module-native-protocol-unix --load="module-always-sink" \
  --log-target=file:"$OUT/pulseaudio.log" 2>/dev/null || true
for i in $(seq 1 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
pactl load-module module-null-sink sink_name=a_to_b rate=$RATE channels=2 >/dev/null
pactl load-module module-null-sink sink_name=b_to_a rate=$RATE channels=2 >/dev/null
pactl set-default-sink auto_null 2>/dev/null || true

write_ini() { # name base bcast
  cat > "$OUT/$1/mercury.ini" <<EOF
[main]
ui_enabled = true
sound_system = pulse
arq_tcp_base_port = $2
broadcast_tcp_port = $3
log_utc = true
[store]
enabled = false
EOF
}
write_ini a 8300 8100
write_ini b 8400 8200

start_rigctld() { # name port
  rigctld -m 1 -P RIG -T 127.0.0.1 -t "$2" >> "$OUT/$1/rigctld.log" 2>&1 &
  echo $! > "$OUT/$1/rigctld.pid"
  for i in $(seq 1 50); do timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$2" 2>/dev/null && break; sleep 0.1; done
}
start_rigctld a 4532
start_rigctld b 4632
# Give the dummy rigs a realistic starting state before Mercury opens them.
rigctl -m 2 -r 127.0.0.1:4532 F 7105000 M PKTUSB 3000
rigctl -m 2 -r 127.0.0.1:4632 F 7105000 M PKTUSB 3000

{
  echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mercury_sha256=$(sha256sum "$BIN" | cut -d' ' -f1)"
  echo "rigctld=$(rigctld --version | head -1)"
  echo "rig_model=1 (Hamlib dummy, -P RIG); Mercury rig model=2 (NET rigctl)"
} > "$OUT/lab-manifest.txt"

"$BIN" -C "$OUT/a/mercury.ini" -x pulse -o a_to_b -i b_to_a.monitor -p 8300 -b 8100 \
  -G -U 10000 -W -R 2 -A 127.0.0.1:4532 -H 2 -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 &
echo $! > "$OUT/a/pid"
"$BIN" -C "$OUT/b/mercury.ini" -x pulse -o b_to_a -i a_to_b.monitor -p 8400 -b 8200 \
  -G -U 10100 -W -R 2 -A 127.0.0.1:4632 -H 2 -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
echo $! > "$OUT/b/pid"
for port in 8300 8301 8400 8401 10000 10100; do
  for i in $(seq 1 100); do timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$port" 2>/dev/null && break; sleep 0.1; done
done
echo "RIG LAB READY"

set +e
python3 /scripts/rig-proof.py "$OUT"
rc=$?
kill "$(cat "$OUT/a/pid")" "$(cat "$OUT/b/pid")" 2>/dev/null
sleep 2
pkill rigctld 2>/dev/null
exit $rc
