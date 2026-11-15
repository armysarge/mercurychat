#!/usr/bin/env bash
# MercuryChat M0 Raspberry Pi proof (docs/07 §7-8). Run ON the Pi, from a
# graphical desktop session (Wayland or X11), as the normal desktop user.
#
#   ./pi-proof.sh [--minutes N] [--skip-lab]
#
# Kit layout (copy this whole folder to the Pi):
#   mercury            linux-arm64 Mercury build (lab/bin/linux-arm64/mercury)
#   m0-client          linux-arm64 test client
#   MercuryChat_*_arm64.deb  linux-arm64 MercuryChat package
#   pi-proof.sh        this script
#
# What it does — nothing transmits RF and no physical PTT is configured:
#   1. records hardware/OS/CPU/thermal/power baseline;
#   2. verifies every binary is aarch64 and its libraries resolve;
#   3. installs the .deb (sudo apt install ./mercurychat_*.deb);
#   4. measures app shell startup (cold-ish and warm) and IPC round trips;
#   5. runs the two-Mercury crossed-audio lab (PipeWire/Pulse null sinks,
#      PTT none) with the m0-client scenario WHILE the app shell is open,
#      sampling CPU/RSS of app, WebKit and Mercury processes and temperature.
# Results go to ./pi-results-<host>-<utc>/. Copy that folder back.
set -uo pipefail

MINUTES=10
SKIP_LAB=0
while [ $# -gt 0 ]; do
  case "$1" in
    --minutes) MINUTES=$2; shift 2 ;;
    --skip-lab) SKIP_LAB=1; shift ;;
    *) echo "unknown arg $1"; exit 2 ;;
  esac
done

KIT=$(cd "$(dirname "$0")" && pwd)
OUT="$KIT/pi-results-$(hostname)-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$OUT"
log() { echo "[$(date -u +%H:%M:%S)] $*" | tee -a "$OUT/run.log"; }

# ---------------------------------------------------------------- 1 baseline
{
  echo "model=$(tr -d '\0' < /proc/device-tree/model 2>/dev/null || echo unknown)"
  echo "mem_total_kb=$(awk '/MemTotal/ {print $2}' /proc/meminfo)"
  echo "kernel=$(uname -srm)"
  . /etc/os-release 2>/dev/null && echo "os=$PRETTY_NAME"
  echo "userland_bits=$(getconf LONG_BIT)"
  echo "cpu_features=$(awk -F: '/Features/ {print $2; exit}' /proc/cpuinfo | xargs)"
  echo "cpu_part=$(awk -F: '/CPU part/ {print $2; exit}' /proc/cpuinfo | xargs)"
  echo "nproc=$(nproc)"
  echo "session_type=${XDG_SESSION_TYPE:-unknown} desktop=${XDG_CURRENT_DESKTOP:-unknown} display=${DISPLAY:-} wayland=${WAYLAND_DISPLAY:-}"
  command -v vcgencmd >/dev/null && echo "throttled_start=$(vcgencmd get_throttled)" && echo "temp_start=$(vcgencmd measure_temp)"
  echo "root_fs=$(findmnt -no SOURCE,FSTYPE /)"
  echo "webkit=$(dpkg-query -W -f='${Version}' libwebkit2gtk-4.1-0 2>/dev/null || echo not-installed)"
  echo "audio_server=$(pactl info 2>/dev/null | awk -F: '/Server Name/ {print $2}' | xargs)"
} > "$OUT/baseline.txt"
log "baseline: $(head -1 "$OUT/baseline.txt")"

# ---------------------------------------------------------- 2 binary checks
for b in mercury m0-client; do
  [ -x "$KIT/$b" ] || chmod +x "$KIT/$b" 2>/dev/null
  { echo "== $b"; file -b "$KIT/$b"; sha256sum "$KIT/$b"; ldd "$KIT/$b" 2>&1 | grep -E 'not found|=>' ; } >> "$OUT/binaries.txt"
done
if grep -q 'not found' "$OUT/binaries.txt"; then log "FAIL: unresolved libraries (see binaries.txt)"; fi
"$KIT/mercury" -V 2>&1 | sed 's/\x1b\[[0-9;]*m//g' | tail -1 >> "$OUT/binaries.txt"

# ------------------------------------------------------------- 3 install app
DEB=$(ls "$KIT"/*_arm64.deb 2>/dev/null | head -1)
if [ -n "$DEB" ]; then
  { dpkg-deb -f "$DEB" Package Version Architecture Depends; sha256sum "$DEB"; } >> "$OUT/binaries.txt"
  if ! command -v mercurychat >/dev/null; then
    log "installing $DEB (sudo)"
    sudo apt-get install -y "$DEB" >> "$OUT/install.log" 2>&1 || log "FAIL: package install (install.log)"
  fi
fi
APP=$(command -v mercurychat || echo /usr/bin/mercurychat)

# ------------------------------------------------------- 4 startup + IPC
export MC_DATA_DIR="$OUT/app-data"
for run in 1 2 3; do
  start=$(date +%s%3N)
  out=$(MC_SELF_MEASURE=1 MC_EXIT_AFTER_READY_MS=1500 timeout 60 "$APP" 2>&1)
  end=$(date +%s%3N)
  echo "run=$run wall_ms=$((end-start)) $(echo "$out" | grep -E 'MC_UI_READY_MS|MC_IPC_MS' | xargs)" >> "$OUT/startup.txt"
done
log "startup: $(cat "$OUT/startup.txt" | tr '\n' ';')"

[ "$SKIP_LAB" = 1 ] && { log "lab skipped"; exit 0; }

# ------------------------------------- 5 crossed-audio lab under app load
# PipeWire (Pi OS default) answers pactl through pipewire-pulse.
pactl load-module module-null-sink sink_name=mc_a_to_b rate=48000 channels=2 > "$OUT/sink-a.id"
pactl load-module module-null-sink sink_name=mc_b_to_a rate=48000 channels=2 > "$OUT/sink-b.id"
cleanup() {
  kill "${PIDS[@]}" 2>/dev/null
  pactl unload-module "$(cat "$OUT/sink-a.id")" 2>/dev/null
  pactl unload-module "$(cat "$OUT/sink-b.id")" 2>/dev/null
}
trap cleanup EXIT
mkdir -p "$OUT/a" "$OUT/b"
for s in a b; do
  printf '[main]\nui_enabled = false\nlog_utc = true\n[ptt]\nmethod = none\n[store]\nenabled = false\n' > "$OUT/$s/mercury.ini"
done
PIDS=()
"$KIT/mercury" -C "$OUT/a/mercury.ini" -x pulse -o mc_a_to_b -i mc_b_to_a.monitor -p 8300 -b 8100 -P none -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 & PIDS+=($!); MA=$!
"$KIT/mercury" -C "$OUT/b/mercury.ini" -x pulse -o mc_b_to_a -i mc_a_to_b.monitor -p 8400 -b 8200 -P none -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 & PIDS+=($!); MB=$!
sleep 3
MC_SELF_MEASURE=1 "$APP" > "$OUT/app-under-load.log" 2>&1 & PIDS+=($!); APPPID=$!
sleep 5

# Resource sampler: whole process trees, separately and together.
( while sleep 2; do
    ts=$(date -u +%H:%M:%S)
    temp=$(vcgencmd measure_temp 2>/dev/null | cut -d= -f2)
    thr=$(vcgencmd get_throttled 2>/dev/null | cut -d= -f2)
    for name in mercurychat WebKitWebProcess WebKitNetworkProcess; do
      ps -C "$name" -o pcpu=,rss= 2>/dev/null | awk -v t="$ts" -v n="$name" '{c+=$1; r+=$2} END {if (NR) print t, n, "cpu%=" c, "rss_kb=" r}'
    done
    ps -o pcpu=,rss= -p "$MA" -p "$MB" | awk -v t="$ts" '{c+=$1; r+=$2} END {print t, "mercury(2)", "cpu%=" c, "rss_kb=" r}'
    echo "$ts temp=$temp throttled=$thr load=$(cut -d' ' -f1-3 /proc/loadavg)"
  done ) > "$OUT/resources.log" 2>&1 & PIDS+=($!)

log "running M0 scenario over the Pi's crossed audio routes (this takes a while)"
timeout $((MINUTES*60 + 3600)) "$KIT/m0-client" lab --a 127.0.0.1:8300 --b 127.0.0.1:8400 --work "$OUT/client" --file-bytes 4096 \
  > "$OUT/client-console.log" 2>&1
log "scenario exit=$? : $(tail -1 "$OUT/client-console.log")"

# Keep sampling idle-with-app for the requested minutes.
log "idle sampling for $MINUTES min with app + modems running"
sleep $((MINUTES*60))
command -v vcgencmd >/dev/null && echo "throttled_end=$(vcgencmd get_throttled) temp_end=$(vcgencmd measure_temp)" >> "$OUT/baseline.txt"
log "done: results in $OUT"
