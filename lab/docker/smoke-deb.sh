#!/usr/bin/env bash
# Clean-system package smoke test: install the .deb on bare debian:trixie,
# check libraries resolve, launch under Xvfb and read the app's timing lines.
set -uo pipefail
export DEBIAN_FRONTEND=noninteractive
deb=$(ls /pkg/*.deb | head -1)
echo "== $(basename "$deb") on $(uname -m)"
dpkg-deb -c "$deb" | awk '{print $6}' | grep -E 'bin/|\.desktop$'
apt-get update -qq >/dev/null && apt-get install -y -qq "$deb" xvfb xauth dbus-x11 >/tmp/apt.log 2>&1 || { tail -20 /tmp/apt.log; exit 1; }
echo "installed: $(dpkg-query -W -f='${Package} ${Version} ${Architecture}' mercurychat)"
echo "unresolved libs: $(ldd /usr/bin/mercurychat | grep -c 'not found')"
export MC_DATA_DIR=/tmp/mc MC_SELF_MEASURE=1 MC_EXIT_AFTER_READY_MS=1500
for i in 1 2; do
  s=$(date +%s%3N)
  out=$(timeout 180 xvfb-run -a dbus-launch --exit-with-session /usr/bin/mercurychat 2>/tmp/err.log)
  echo "run $i: exit=$? wall_ms=$(( $(date +%s%3N) - s )) $(echo "$out" | grep -E 'MC_' | xargs)"
done
grep -viE 'libEGL|MESA|dri|ZINK|glx' /tmp/err.log | tail -5
