#!/usr/bin/env bash
# Clean-system package smoke test: install the .deb on a bare base image,
# check libraries resolve, launch under Xvfb and read the app's timing lines.
#
# On the host:  bash lab/docker/smoke-deb.sh [image] [package dir]
#   image        default debian:trixie (e.g. ubuntu:24.04)
#   package dir  default lab/bin/app-linux/amd64 (the first *.deb in it)
# The script re-runs itself inside a fresh container of that image.
set -uo pipefail

if [ -z "${MC_SMOKE_INNER:-}" ]; then
  image=${1:-debian:trixie}
  here=$(cd "$(dirname "$0")" && pwd)
  pkg=$(cd "${2:-$here/../bin/app-linux/amd64}" && pwd)
  win() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
  MSYS_NO_PATHCONV=1 exec docker run --rm -e MC_SMOKE_INNER=1 \
    -v "$(win "$pkg"):/pkg:ro" -v "$(win "$here"):/s:ro" "$image" bash /s/smoke-deb.sh
fi

export DEBIAN_FRONTEND=noninteractive
deb=$(ls /pkg/*.deb | head -1)
echo "== $(basename "$deb") on $(. /etc/os-release && echo "$PRETTY_NAME") $(uname -m)"
echo "sha256: $(sha256sum "$deb" | cut -d' ' -f1)"
echo "Depends: $(dpkg-deb -f "$deb" Depends)"
dpkg-deb -c "$deb" | awk '{print $6}' | grep -E 'bin/|\.desktop$'
apt-get update -qq >/dev/null && apt-get install -y -qq "$deb" xvfb xauth dbus-x11 >/tmp/apt.log 2>&1 || { tail -20 /tmp/apt.log; exit 1; }
echo "installed: $(dpkg-query -W -f='${Package} ${Version} ${Architecture}' "$(dpkg-deb -f "$deb" Package)")"
echo "unresolved libs: $(ldd /usr/bin/mercurychat | grep -c 'not found')"
export MC_DATA_DIR=/tmp/mc MC_SELF_MEASURE=1 MC_EXIT_AFTER_READY_MS=1500
for i in 1 2; do
  s=$(date +%s%3N)
  out=$(timeout 180 xvfb-run -a dbus-launch --exit-with-session /usr/bin/mercurychat 2>/tmp/err.log)
  echo "run $i: exit=$? wall_ms=$(( $(date +%s%3N) - s )) $(echo "$out" | grep -E 'MC_' | xargs)"
done
grep -viE 'libEGL|MESA|dri|ZINK|glx' /tmp/err.log | tail -5
