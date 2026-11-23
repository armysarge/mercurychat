#!/usr/bin/env bash
# Clean install of a MercuryChat release package on a bare distro image
# (M5 Task 12). Exits non-zero unless every one of these holds:
#   I1 the package installs with only the distro's own repositories
#   I2 /usr/lib/mercurychat/mercury -h and rigctld --version run (no missing library)
#   I3 mercurychat --version runs headless (no display) and names the versions
#   I4 the built-in modem's manifest verifies: MANIFEST.sha256 in
#      /usr/lib/mercurychat checks, and the app itself reports "bundle: verified"
#   I5 upgrade_keeps_data: install N, write a marker in the app's data folder,
#      install N+1 over it, the marker is still there and N+1 runs
#
# On the host:  bash lab/release/clean-install.sh <image> <package file> [<older package file>]
#   image     ubuntu:22.04, ubuntu:24.04, debian:12, archlinux, ...
#   package   a .deb or an Arch .pkg.tar.zst (N+1)
#   older     N; without one, N is the same package repacked with version 0.0.1
#             (the package manager still upgrades over it; Ruling, Task 12)
# The script re-runs itself inside a fresh container of that image.
set -uo pipefail

if [ -z "${MC_CLEAN_INNER:-}" ]; then
  image=${1:-}; pkg=${2:-}; old=${3:-}
  if [ -z "$image" ] || [ -z "$pkg" ] || [ ! -f "$pkg" ]; then
    echo "FAIL usage: clean-install.sh <image> <package file> [<older package file>] (package missing: '${pkg}')" >&2
    exit 2
  fi
  here=$(cd "$(dirname "$0")" && pwd)
  pdir=$(cd "$(dirname "$pkg")" && pwd); pname=$(basename "$pkg")
  odir=$pdir; oname=""
  if [ -n "$old" ]; then odir=$(cd "$(dirname "$old")" && pwd); oname=$(basename "$old"); fi
  win() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }
  MSYS_NO_PATHCONV=1 exec docker run --rm -e MC_CLEAN_INNER=1 -e PKG="/pkg/$pname" -e OLD="${oname:+/old/$oname}" \
    -v "$(win "$pdir"):/pkg:ro" -v "$(win "$odir"):/old:ro" -v "$(win "$here"):/s:ro" "$image" bash /s/clean-install.sh
fi

fails=0
pass() { echo "PASS $*"; }
fail() { echo "FAIL $*"; fails=$((fails + 1)); }
. /etc/os-release
echo "== $(basename "$PKG") on $PRETTY_NAME $(uname -m)"

case "$PKG" in
  *.deb) kind=deb ;;
  *.pkg.tar.zst) kind=arch ;;
  *) echo "FAIL not a .deb or .pkg.tar.zst: $PKG"; exit 1 ;;
esac

prepare() {
  if [ $kind = deb ]; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
  else
    pacman -Sy --noconfirm >/dev/null 2>&1
  fi
}
install_pkg() {
  if [ $kind = deb ]; then
    apt-get install -y -qq --allow-downgrades "$1" >/tmp/install.log 2>&1
  else
    pacman -U --noconfirm "$1" >/tmp/install.log 2>&1
  fi
}
# The package's own name (Tauri names the .deb "mercury-chat"; the Arch one is "mercurychat").
if [ $kind = deb ]; then name=$(dpkg-deb -f "$PKG" Package); else name=$(tar -xOf "$PKG" .PKGINFO | sed -n 's/^pkgname = //p'); fi
installed_version() {
  if [ $kind = deb ]; then dpkg-query -W -f='${Version}' "$name" 2>/dev/null; else pacman -Q "$name" 2>/dev/null | cut -d' ' -f2; fi
}
# N: the given older package, else this one repacked as version 0.0.1.
make_older() {
  mkdir -p /tmp/older && cd /tmp/older || return 1
  if [ $kind = deb ]; then
    dpkg-deb -R "$PKG" x && sed -i 's/^Version: .*/Version: 0.0.1/' x/DEBIAN/control && dpkg-deb -b x older.deb >/dev/null && echo /tmp/older/older.deb
  else
    pacman -S --noconfirm --needed zstd >/dev/null 2>&1
    mkdir x && tar -C x -xf "$PKG" && sed -i 's/^pkgver = .*/pkgver = 0.0.1-1/' x/.PKGINFO \
      && (cd x && tar --zstd -cf ../mercurychat-0.0.1-1-x86_64.pkg.tar.zst .PKGINFO $(ls -A | grep -v '^\.PKGINFO$')) \
      && echo /tmp/older/mercurychat-0.0.1-1-x86_64.pkg.tar.zst
  fi
}

prepare

# ---- I5 part 1: N first, a marker in its data folder.
older=${OLD:-}
[ -z "$older" ] && older=$(make_older)
data="$HOME/.local/share/org.mercurychat.desktop"
if [ -n "$older" ] && install_pkg "$older"; then
  mkdir -p "$data" && echo "kept $(date -u +%FT%TZ)" > "$data/upgrade-marker.txt"
  n_version=$(installed_version)
  [ -n "$n_version" ] || fail "I5 N installed but $name reports no version"
else
  fail "I5 the older package N did not install: $(tail -5 /tmp/install.log 2>/dev/null | xargs)"
  n_version=""
fi

# ---- I1: N+1 installs from the distro's repositories alone.
if install_pkg "$PKG"; then
  pass "I1 installed $(installed_version) with $PRETTY_NAME's repositories only"
else
  fail "I1 install: $(tail -8 /tmp/install.log | xargs)"
  echo "RESULT FAIL ($fails)"; exit 1
fi

# ---- I2: the built-in modem's programs run.
lib=/usr/lib/mercurychat
out=$("$lib/mercury" -h 2>&1); rc=$?
if [ $rc -ne 126 ] && [ $rc -ne 127 ] && ! grep -q "error while loading shared libraries" <<<"$out" && [ -n "$out" ]; then
  pass "I2 mercury -h runs (exit $rc): $(head -1 <<<"$out" | cut -c1-80)"
else
  fail "I2 mercury -h (exit $rc): $(head -3 <<<"$out" | xargs)"
fi
out=$("$lib/rigctld" --version 2>&1); rc=$?
if [ $rc -eq 0 ]; then pass "I2 rigctld --version: $(head -1 <<<"$out")"; else fail "I2 rigctld --version (exit $rc): $(head -3 <<<"$out" | xargs)"; fi
missing=$(for f in "$lib/mercury" "$lib/rigctld" /usr/bin/mercurychat; do ldd "$f" 2>/dev/null | grep 'not found'; done | sort -u | xargs)
if [ -z "$missing" ]; then pass "I2 every library resolves"; else fail "I2 unresolved libraries: $missing"; fi

# ---- I3 and I4: the app headless, and the manifest.
unset DISPLAY WAYLAND_DISPLAY
out=$(timeout 30 mercurychat --version 2>&1); rc=$?
if [ $rc -eq 0 ] && grep -q "^MercuryChat " <<<"$out"; then
  pass "I3 mercurychat --version headless: $(xargs <<<"$out")"
else
  fail "I3 mercurychat --version (exit $rc): $(head -5 <<<"$out" | xargs)"
fi
if grep -q "^bundle: verified" <<<"$out"; then pass "I4 the app verifies its bundle"; else fail "I4 the app does not report 'bundle: verified'"; fi
if (cd "$lib" && sha256sum -c --quiet MANIFEST.sha256 2>&1); then pass "I4 MANIFEST.sha256 checks in $lib"; else fail "I4 MANIFEST.sha256 does not check in $lib"; fi

# ---- I5 part 2: the marker survived the upgrade.
if [ -n "$n_version" ]; then
  if [ -f "$data/upgrade-marker.txt" ]; then
    pass "I5 upgrade_keeps_data: $n_version -> $(installed_version), marker kept ($(cat "$data/upgrade-marker.txt"))"
  else
    fail "I5 upgrade_keeps_data: the marker is gone after $n_version -> $(installed_version)"
  fi
else
  fail "I5 upgrade_keeps_data did not run (no N installed)"
fi

if [ $fails -eq 0 ]; then echo "RESULT PASS"; exit 0; fi
echo "RESULT FAIL ($fails)"; exit 1
