#!/usr/bin/env bash
# Desktop shell alongside both Windows modems (M0 item 5's desktop analogue).
# Starts the Windows cable lab (run-win-lab.sh), and while the M0 scenario is
# exchanging traffic:
#   * keeps one MercuryChat window open for the whole scenario (own data dir);
#   * launches the shell 4 more times with MC_SELF_MEASURE, one per minute,
#     recording first-render time and IPC round trips under modem load;
#   * samples the app process tree and both modems every 5 s.
# The shell never attaches to Mercury (modem_probe is a manual action), so the
# one-client rule is not touched. Run from the repo root in Git Bash.
set -uo pipefail

OUT=${OUT:-lab/run/win-shell-load}
APP=${APP:-target/release/mercurychat.exe}
# App data roots; put them on the same disk a real install would use (default: under OUT).
DATA=${DATA:-$OUT}
mkdir -p "$OUT" "$DATA"
rm -f "$OUT/stop"

OUT="$OUT/lab" bash lab/windows/run-win-lab.sh > "$OUT/lab-console.log" 2>&1 &
LAB=$!

powershell -NoProfile -ExecutionPolicy Bypass -File lab/windows/sample-tree.ps1 \
  -Out "$(cygpath -w "$OUT/tree.log")" -StopFile "$(cygpath -w "$OUT/stop")" &
SAMPLER=$!

# Wait for the scenario's radio session to come up.
for i in $(seq 1 600); do grep -q 'CONNECTED LABA LABB' "$OUT/lab-console.log" 2>/dev/null && break; sleep 0.5; done
echo "$(date -u +%H:%M:%S) session up; opening long-lived window" | tee -a "$OUT/timeline.log"

MC_DATA_DIR="$(cygpath -w "$DATA/data-long")" MC_SELF_MEASURE=1 "$APP" > "$OUT/long.out" 2> "$OUT/long.err" &
LONG=$!

for n in 1 2 3 4; do
  sleep 60
  kill -0 "$LAB" 2>/dev/null || break
  echo "$(date -u +%H:%M:%S) short launch $n" | tee -a "$OUT/timeline.log"
  MC_DATA_DIR="$(cygpath -w "$DATA/data-$n")" MC_SELF_MEASURE=1 MC_EXIT_AFTER_READY_MS=2500 \
    timeout 60 "$APP" > "$OUT/short-$n.out" 2> "$OUT/short-$n.err"
done

wait "$LAB"; LAB_RC=$?
echo "$(date -u +%H:%M:%S) scenario finished rc=$LAB_RC; closing long-lived window" | tee -a "$OUT/timeline.log"
sleep 10   # one more idle sample with the window open
taskkill //PID "$(ps -p "$LONG" -o winpid= | tr -d ' ')" > /dev/null 2>&1 || kill "$LONG" 2>/dev/null
sleep 3
kill -0 "$LONG" 2>/dev/null && taskkill //F //PID "$(ps -p "$LONG" -o winpid= | tr -d ' ')" > /dev/null 2>&1
touch "$OUT/stop"; wait "$SAMPLER" 2>/dev/null
echo "done rc=$LAB_RC" | tee -a "$OUT/timeline.log"
exit $LAB_RC
