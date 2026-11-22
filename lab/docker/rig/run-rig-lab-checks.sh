#!/usr/bin/env bash
# Rig lab checks: the real MercuryChat service (rig_lab) against real Mercury
# and Hamlib. Usage: run-rig-lab-checks.sh [m3b|m3b1|m3c] (default m3b).
#
# m3c (M3c Task 14; M3c-checks.md): QSY, its fallback and gate, info
# requests and MCB/1 kinds 2 and 3 between two MercuryChat instances.
#
# m3b1 (M3b.1 Task 9; what each check proves is in M3b1-checks.md): Mercury A
# starts with -P none and no rigctld on 4532; MercuryChat starts its own
# (managed, dummy rig), pushes Mercury's PTT, shows the offset and sets
# Mercury's audio; then a rigctld already on 4532 is reused and left running.
#
# m3b: the M3b rig lab exit (docs/M3b-plan.md Task 12). The rig lab's crossed
# null-sink audio and two Mercury stations keying PTT through their own
# Hamlib rigctld (dummy rig, model 1: no transceiver, no serial port, no RF),
# driven by the real MercuryChat service actor, headless (rig_lab):
#
#   Station A (ours): rigctld 127.0.0.1:4532 (-vvvv log), Mercury -R 2 -A 127.0.0.1:4532, TNC 8300, WS 10000
#   Station B (peer): rigctld 127.0.0.1:4632, Mercury -R 2 -A 127.0.0.1:4632, TNC 8400, WS 10100
#   Station C (check 9 only): Mercury configured with a serial Hamlib model
#     (-R 2014, TS-2000) on /tmp/ttyV0, one end of a pseudo-terminal pair
#     (pty-pair.py) whose other end Hamlib's rigctlcom answers as a TS-2000
#     backed by the dummy rig. No real serial device. TNC 8500, WS 10200
#
# Mount the Mercury build at /mercury and rig_lab at /lab; output in /out.
set -euo pipefail

MODE=${1:-m3b}
BIN=${BIN:-/mercury/linux-amd64/mercury}
LAB=${LAB:-/lab/rig_lab}
OUT=${OUT:-/out}
RATE=48000
mkdir -p "$OUT/a" "$OUT/b" "$OUT/c"

export XDG_RUNTIME_DIR=/tmp/pa-runtime; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
pulseaudio --daemonize=yes --exit-idle-time=-1 --disallow-exit=yes --system=no \
  -n --load=module-native-protocol-unix --load="module-always-sink" \
  --log-target=file:"$OUT/pulseaudio.log" 2>/dev/null || true
for i in $(seq 1 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
pactl load-module module-null-sink sink_name=a_to_b rate=$RATE channels=2 >/dev/null
pactl load-module module-null-sink sink_name=b_to_a rate=$RATE channels=2 >/dev/null
pactl load-module module-null-sink sink_name=c_out rate=$RATE channels=2 >/dev/null
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
write_ini c 8500 8250

wait_port() { for i in $(seq 1 100); do timeout 1 bash -c "exec 3<>/dev/tcp/127.0.0.1/$1" 2>/dev/null && return 0; sleep 0.1; done; return 1; }

if [ "$MODE" = m3b1 ]; then
  # Station B keys its own rigctld as in m3b; station A has none until MercuryChat starts one.
  rigctld -m 1 -P RIG -T 127.0.0.1 -t 4632 >> "$OUT/b/rigctld.log" 2>&1 &
  wait_port 4632
  rigctl -m 2 -r 127.0.0.1:4632 F 7105000 M PKTUSB 3000
  {
    echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ) mode=m3b1"
    echo "mercury_sha256=$(sha256sum "$BIN" | cut -d' ' -f1)"
    cat "$(dirname "$BIN")/MANIFEST.txt" 2>/dev/null | grep -E '^(mercury_commit|patches)=' || true
    echo "rig_lab_sha256=$(sha256sum "$LAB" | cut -d' ' -f1)"
    echo "rigctld=$(rigctld --version | head -1)"
    echo "station A: Mercury -P none, no rigctld until MercuryChat's (dummy rig, model 1); station B: Mercury -R 2 -A 127.0.0.1:4632"
  } > "$OUT/lab-manifest.txt"
  "$BIN" -C "$OUT/a/mercury.ini" -x pulse -o a_to_b -i b_to_a.monitor -p 8300 -b 8100 \
    -G -U 10000 -W -P none -H 2 -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 &
  "$BIN" -C "$OUT/b/mercury.ini" -x pulse -o b_to_a -i a_to_b.monitor -p 8400 -b 8200 \
    -G -U 10100 -W -R 2 -A 127.0.0.1:4632 -H 2 -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
  for port in 8300 8301 8400 8401 10000 10100; do wait_port $port; done
  echo "M3B1 RIG LAB READY"
  export WS_SNOOP="python3 /scripts/ws-snoop.py 10000 2"
  export MERCURY_A_INI="$OUT/a/mercury.ini"
  on_4532() { pgrep -a rigctld | grep -- '-t 4532' || true; }

  set +e
  lines=()
  RUST_LOG=${RUST_LOG:-info} "$LAB" "$OUT" m3b1 2> "$OUT/rig_lab-m3b1.stderr.log" | tee "$OUT/rig_lab-m3b1.stdout.log"
  sleep 1
  left=$(on_4532)
  if [ -z "$left" ]; then lines+=("PASS 1 managed, after exit: no rigctld left on 4532")
  else lines+=("FAIL 1 managed, after exit: still running: $left"); kill $(echo "$left" | awk '{print $1}'); sleep 0.5; fi

  rigctld -m 1 -P RIG -T 127.0.0.1 -t 4532 >> "$OUT/a/rigctld-reused.log" 2>&1 &
  wait_port 4532
  RUST_LOG=${RUST_LOG:-info} "$LAB" "$OUT" reuse 2> "$OUT/rig_lab-reuse.stderr.log" | tee "$OUT/rig_lab-reuse.stdout.log"
  sleep 1
  kept=$(on_4532)
  if [ -n "$kept" ]; then lines+=("PASS 2 reuse, after exit: the reused rigctld still runs ($kept)")
  else lines+=("FAIL 2 reuse, after exit: the reused rigctld was stopped"); fi

  {
    grep -hE '^(PASS|FAIL) ' "$OUT/report-m3b1.txt" "$OUT/report-reuse.txt" 2>/dev/null
    printf '%s\n' "${lines[@]}"
  } > "$OUT/report.txt"
  pass=$(grep -c '^PASS' "$OUT/report.txt"); total=$(grep -cE '^(PASS|FAIL)' "$OUT/report.txt")
  echo "$pass of $total passed" >> "$OUT/report.txt"
  echo "=== report"; cat "$OUT/report.txt"
  pkill -f "$BIN" 2>/dev/null
  sleep 2
  pkill rigctld 2>/dev/null
  [ "$pass" = "$total" ] && [ "$total" -ge 7 ]
  exit $?
fi

if [ "$MODE" = m3c ]; then
  # M3c (M3c-checks.md): two MercuryChat instances (rig_lab m3c-qsy), each
  # with its own dummy rigctld that Mercury also keys through. No RF.
  export RIG_LOG="$OUT/a/rigctld.log"
  export RIG_START="rigctld -m 1 -P RIG -T 127.0.0.1 -t 4532 >> $RIG_LOG 2>&1 & echo \$! > $OUT/a/rigctld.pid; for i in \$(seq 1 50); do (exec 3<>/dev/tcp/127.0.0.1/4532) 2>/dev/null && break; sleep 0.1; done; echo '--- rigctld A started with -P RIG' >> $OUT/a/events.log"
  export RIG_START_NOP="rigctld -m 1 -T 127.0.0.1 -t 4532 >> $RIG_LOG 2>&1 & echo \$! > $OUT/a/rigctld.pid; for i in \$(seq 1 50); do (exec 3<>/dev/tcp/127.0.0.1/4532) 2>/dev/null && break; sleep 0.1; done; echo '--- rigctld A started without -P' >> $OUT/a/events.log"
  export RIG_KILL="kill \$(cat $OUT/a/rigctld.pid); sleep 0.5; echo '--- rigctld A killed' >> $OUT/a/events.log"
  bash -c "$RIG_START"
  rigctld -m 1 -P RIG -T 127.0.0.1 -t 4632 >> "$OUT/b/rigctld.log" 2>&1 &
  wait_port 4632
  rigctl -m 2 -r 127.0.0.1:4532 F 7105000 M USB 2400
  rigctl -m 2 -r 127.0.0.1:4632 F 7105000 M USB 2400
  {
    echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ) mode=m3c"
    echo "mercury_sha256=$(sha256sum "$BIN" | cut -d' ' -f1)"
    cat "$(dirname "$BIN")/MANIFEST.txt" 2>/dev/null | grep -E '^(mercury_commit|patches)=' || true
    echo "rig_lab_sha256=$(sha256sum "$LAB" | cut -d' ' -f1)"
    echo "rigctld=$(rigctld --version | head -1)"
    echo "station A: Mercury -R 2 -A 127.0.0.1:4532, offset 0; station B: Mercury -R 2 -A 127.0.0.1:4632, offset +1500 (dummy rigs, model 1)"
  } > "$OUT/lab-manifest.txt"
  "$BIN" -C "$OUT/a/mercury.ini" -x pulse -o a_to_b -i b_to_a.monitor -p 8300 -b 8100 \
    -G -U 10000 -W -R 2 -A 127.0.0.1:4532 -H 2 -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 &
  "$BIN" -C "$OUT/b/mercury.ini" -x pulse -o b_to_a -i a_to_b.monitor -p 8400 -b 8200 \
    -G -U 10100 -W -R 2 -A 127.0.0.1:4632 -H 2 -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
  for port in 8300 8301 8100 8400 8401 8200 10000 10100; do wait_port $port; done
  echo "M3C RIG LAB READY"
  set +e
  RUST_LOG=${RUST_LOG:-info} "$LAB" "$OUT" m3c-qsy 2> "$OUT/rig_lab-m3c.stderr.log" | tee "$OUT/rig_lab-m3c.stdout.log"
  rc=${PIPESTATUS[0]}
  cp "$OUT/report-m3c-qsy.txt" "$OUT/report.txt" 2>/dev/null
  pkill -f "$BIN" 2>/dev/null
  sleep 2
  pkill rigctld 2>/dev/null
  exit $rc
fi

# Station A's rigctld: rig_lab kills and restarts it through these.
export RIG_LOG="$OUT/a/rigctld.log"
export RIG_START="rigctld -m 1 -P RIG -T 127.0.0.1 -t 4532 -vvvv >> $RIG_LOG 2>&1 & echo \$! > $OUT/a/rigctld.pid; for i in \$(seq 1 50); do (exec 3<>/dev/tcp/127.0.0.1/4532) 2>/dev/null && break; sleep 0.1; done; echo '--- rigctld A started with -P RIG' >> $OUT/a/events.log"
export RIG_START_NOP="rigctld -m 1 -T 127.0.0.1 -t 4532 -vvvv >> $RIG_LOG 2>&1 & echo \$! > $OUT/a/rigctld.pid; for i in \$(seq 1 50); do (exec 3<>/dev/tcp/127.0.0.1/4532) 2>/dev/null && break; sleep 0.1; done; echo '--- rigctld A started without -P' >> $OUT/a/events.log"
export RIG_KILL="kill \$(cat $OUT/a/rigctld.pid); sleep 0.5; echo '--- rigctld A killed' >> $OUT/a/events.log"
export MERCURY_C_WS=10200
export MERCURY_C_START="$BIN -C $OUT/c/mercury.ini -x pulse -o c_out -i c_out.monitor -p 8500 -b 8250 -G -U 10200 -W -R 2014 -A /tmp/ttyV0 -H 2 -L $OUT/c/mercury.log > $OUT/c/stdout.log 2>&1 & for p in 8500 8501 10200; do for i in \$(seq 1 100); do (exec 3<>/dev/tcp/127.0.0.1/\$p) 2>/dev/null && break; sleep 0.1; done; done"

python3 /scripts/pty-pair.py &
for i in $(seq 1 50); do [ -e /tmp/ttyV1 ] && break; sleep 0.1; done
rigctlcom -m 1 -R /tmp/ttyV1 -S 9600 > "$OUT/c/rigctlcom.log" 2>&1 &
bash -c "$RIG_START"
rigctld -m 1 -P RIG -T 127.0.0.1 -t 4632 >> "$OUT/b/rigctld.log" 2>&1 &
wait_port 4632
rigctl -m 2 -r 127.0.0.1:4532 F 7105000 M PKTUSB 3000
rigctl -m 2 -r 127.0.0.1:4632 F 7105000 M PKTUSB 3000

{
  echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mercury_sha256=$(sha256sum "$BIN" | cut -d' ' -f1)"
  echo "rig_lab_sha256=$(sha256sum "$LAB" | cut -d' ' -f1)"
  echo "rigctld=$(rigctld --version | head -1)"
  echo "rig_model=1 (Hamlib dummy); Mercury A/B rig model=2 (NET rigctl); Mercury C rig model=2014 (TS-2000) on a pty answered by rigctlcom"
} > "$OUT/lab-manifest.txt"

"$BIN" -C "$OUT/a/mercury.ini" -x pulse -o a_to_b -i b_to_a.monitor -p 8300 -b 8100 \
  -G -U 10000 -W -R 2 -A 127.0.0.1:4532 -H 2 -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 &
"$BIN" -C "$OUT/b/mercury.ini" -x pulse -o b_to_a -i a_to_b.monitor -p 8400 -b 8200 \
  -G -U 10100 -W -R 2 -A 127.0.0.1:4632 -H 2 -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
for port in 8300 8301 8400 8401 10000 10100; do wait_port $port; done
echo "M3B RIG LAB READY"

set +e
RUST_LOG=${RUST_LOG:-info} "$LAB" "$OUT" m3b 2> "$OUT/rig_lab.stderr.log" | tee "$OUT/rig_lab.stdout.log"
rc=${PIPESTATUS[0]}
pkill -f "$BIN" 2>/dev/null
sleep 2
pkill rigctld 2>/dev/null
exit $rc
