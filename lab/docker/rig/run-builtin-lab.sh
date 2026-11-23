#!/usr/bin/env bash
# M5 built-in lab (M5 Task 6; what each check proves is in M5-checks.md): the
# real MercuryChat service, headless (builtin_lab), running its OWN Mercury
# and rigctld from the Linux bundle: Hamlib's dummy rig (model 1), crossed
# PulseAudio null sinks. No transceiver, no serial port, no RF.
#
#   check 3 (here): a service killed hard leaves no Mercury or rigctld behind
#   checks 1, 2, 4 (builtin_lab pair): both READY on loopback only; a session
#     and a 4 KiB file; a port held by another process is named, not adopted
#
# Mounts: /bundle (build-bundle.sh's linux-amd64 folder), /lab (builtin_lab
# from build-builtin-lab.sh), /out. Image: mercurychat-lab:rig.
set -euo pipefail

LAB=${LAB:-/lab/builtin_lab}
OUT=${OUT:-/out}
RATE=48000
export MC_BUNDLE_DIR=${MC_BUNDLE_DIR:-/bundle}
mkdir -p "$OUT"

export XDG_RUNTIME_DIR=/tmp/pa-runtime; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
pulseaudio --daemonize=yes --exit-idle-time=-1 --disallow-exit=yes --system=no \
  -n --load=module-native-protocol-unix --load="module-always-sink" \
  --log-target=file:"$OUT/pulseaudio.log" 2>/dev/null || true
for i in $(seq 1 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
pactl load-module module-null-sink sink_name=a_to_b rate=$RATE channels=2 >/dev/null
pactl load-module module-null-sink sink_name=b_to_a rate=$RATE channels=2 >/dev/null
pactl set-default-sink auto_null 2>/dev/null || true

{
  echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  cat "$MC_BUNDLE_DIR/BUILD.txt"
  echo "sha256_builtin_lab=$(sha256sum "$LAB" | cut -d' ' -f1)"
} > "$OUT/lab-manifest.txt"

count() { { pgrep -x "$1" || true; } | wc -l; }
results="$OUT/results.txt"; : > "$results"

# ---- 3: killed hard, nothing of ours is left.
"$LAB" hold "$OUT/hold" > "$OUT/hold.log" 2>&1 &
pid=$!
for i in $(seq 1 900); do
  grep -q "HOLD READY" "$OUT/hold.log" && break
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.1
done
ready=$(grep -c "HOLD READY" "$OUT/hold.log" || true)
m_before=$(count mercury); r_before=$(count rigctld)
kill -9 "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
sleep 3
m_after=$(count mercury); r_after=$(count rigctld)
detail="ready=$ready; before kill mercury=$m_before rigctld=$r_before; 3 s after kill -9 mercury=$m_after rigctld=$r_after"
if [ "$ready" -ge 1 ] && [ "$m_before" -ge 1 ] && [ "$r_before" -ge 1 ] && [ "$m_after" -eq 0 ] && [ "$r_after" -eq 0 ]; then
  echo "PASS 3 killed service leaves nothing behind: $detail" | tee -a "$results"
else
  echo "FAIL 3 killed service leaves nothing behind: $detail" | tee -a "$results"
fi
pkill -x mercury 2>/dev/null || true; pkill -x rigctld 2>/dev/null || true; sleep 1

# ---- 1, 2, 4.
set +e
"$LAB" pair "$OUT/pair" > "$OUT/pair.log" 2>&1
set -e
grep -E '^(PASS|FAIL) ' "$OUT/pair.log" | tee -a "$results" || true

pass=$(grep -c '^PASS ' "$results" || true)
echo "$pass of 4" | tee "$OUT/summary.txt"
sort -k2 "$results" >> "$OUT/summary.txt"
[ "$pass" -eq 4 ]
