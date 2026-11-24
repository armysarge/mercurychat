#!/usr/bin/env bash
# The Arch package, inside mercurychat-arch-build. Mounts: /pkg (the release
# .deb, ro), /packaging (packaging/arch, ro), /out. Usage: release-arch.sh <deb name> <version>
set -euo pipefail
deb=$1; version=$2
rm -rf /tmp/b && mkdir -p /tmp/b
cp /packaging/PKGBUILD /tmp/b/ && cp "/pkg/$deb" /tmp/b/mercurychat.deb
# Arch versions take no '-': "0.5.0-beta.1" becomes 0.5.0beta.1, which pacman
# sorts before 0.5.0 (a letter right after the number is older: see vercmp(8)).
pkgver=${version//-/}
[ "$version" = "${version%%-*}" ] || [ "$(vercmp "$pkgver" "${version%%-*}")" = -1 ] || { echo "FAIL $pkgver does not sort before ${version%%-*}" >&2; exit 1; }
sed -i "s/^pkgver=.*/pkgver=$pkgver/" /tmp/b/PKGBUILD
chown -R builder /tmp/b
su builder -c "cd /tmp/b && makepkg --nodeps -f" 
mkdir -p /out/arch && cp /tmp/b/*.pkg.tar.zst /out/arch/
ls -l /out/arch
