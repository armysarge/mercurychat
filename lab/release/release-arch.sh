#!/usr/bin/env bash
# The Arch package, inside mercurychat-arch-build. Mounts: /pkg (the release
# .deb, ro), /packaging (packaging/arch, ro), /out. Usage: release-arch.sh <deb name> <version>
set -euo pipefail
deb=$1; version=$2
rm -rf /tmp/b && mkdir -p /tmp/b
cp /packaging/PKGBUILD /tmp/b/ && cp "/pkg/$deb" /tmp/b/mercurychat.deb
sed -i "s/^pkgver=.*/pkgver=${version//-/}/" /tmp/b/PKGBUILD
chown -R builder /tmp/b
su builder -c "cd /tmp/b && makepkg --nodeps -f" 
mkdir -p /out/arch && cp /tmp/b/*.pkg.tar.zst /out/arch/
ls -l /out/arch
