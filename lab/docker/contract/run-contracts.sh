#!/usr/bin/env bash
# Run the contract suite against two real Mercury stations, inside
# mercurychat-lab:rig. Crossed PulseAudio null sinks as in the rig lab; PTT
# none on both stations (no transceiver, no serial port, no RF). A rigctld
# with the dummy rig (model 1) on 4532 serves the one contract that switches
# station A to Hamlib PTT.
#
#   Station A: N0CTA, TNC 8300/8301, broadcast 8100, WS 10000
#   Station B: N0CTB, TNC 8400/8401, broadcast 8200, WS 10100
#
# Mounts: /mercury (build-mercury.sh output), /contracts (build-contracts.sh
# output), /scripts (this directory), /out.
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
  cat > "$OUT/$1/mercury.ini" <<INI
[main]
ui_enabled = true
sound_system = pulse
arq_tcp_base_port = $2
broadcast_tcp_port = $3
log_utc = true
[store]
enabled = false
INI
}
write_ini a 8300 8100
write_ini b 8400 8200

wait_port() { for i in $(seq 1 100); do timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$1" 2>/dev/null && return 0; sleep 0.1; done; return 1; }

rigctld -m 1 -P RIG -T 127.0.0.1 -t 4532 >> "$OUT/a/rigctld.log" 2>&1 &
wait_port 4532

"$BIN" -C "$OUT/a/mercury.ini" -x pulse -o a_to_b -i b_to_a.monitor -p 8300 -b 8100 \
  -G -U 10000 -W -P none -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 &
"$BIN" -C "$OUT/b/mercury.ini" -x pulse -o b_to_a -i a_to_b.monitor -p 8400 -b 8200 \
  -G -U 10100 -W -P none -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
for port in 8300 8301 8100 8400 8401 8200 10000 10100; do wait_port $port || { echo "port $port never opened" >&2; exit 1; }; done

manifest="$(dirname "$BIN")/MANIFEST.txt"
commit=$(grep '^mercury_commit=' "$manifest" | cut -d= -f2)
patches=$(grep '^patches=' "$manifest" | cut -d= -f2-)
{
  echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mercury_commit=$commit"
  echo "patches=$patches"
  echo "mercury_sha256=$(sha256sum "$BIN" | cut -d' ' -f1)"
  grep '^sha256_contracts=' /contracts/MANIFEST.txt
  echo "rigctld=$(rigctld --version | head -1)"
} > "$OUT/lab-manifest.txt"
echo "CONTRACT LAB READY"

export MC_CONTRACT_TARGET=real
export MC_A_BASE=8300 MC_B_BASE=8400 MC_A_BCAST=8100 MC_B_BCAST=8200
export MC_A_WS=ws://127.0.0.1:10000/websocket MC_B_WS=ws://127.0.0.1:10100/websocket
export MC_A_CALL=N0CTA MC_B_CALL=N0CTB
export MC_A_RIGCTLD=127.0.0.1:4532 MC_A_INI="$OUT/a/mercury.ini"
export MC_DIVERGENCES=/scripts/divergences.txt

set +e
/contracts/contracts --test-threads=1 --nocapture > "$OUT/contracts.log" 2>&1
rc=$?
set -e

{
  echo "mercury_commit=$commit"
  echo "patches=$patches"
  # With --nocapture a line can start with the harness's "test <path> ... ".
  sed -nE 's/^(test .* \.\.\. )?CONTRACT ([a-z0-9_]+) (PASS|SKIP|DIVERGENCE|FAIL).*/\3 \2/p' "$OUT/contracts.log"
  sed -nE 's/^(test .* \.\.\. )?DIVERGENCE-FIXED ([a-z0-9_]+).*/FAIL \2 (listed as a divergence but passes)/p' "$OUT/contracts.log"
  grep -E '^test result:' "$OUT/contracts.log" || echo "test result: none (the binary exited $rc)"
} > "$OUT/summary.txt"

pkill -f "$BIN" 2>/dev/null || true
sleep 2
pkill rigctld 2>/dev/null || true
if [ "$rc" -ne 0 ] || grep -q '^FAIL ' "$OUT/summary.txt"; then exit 1; fi
exit 0
