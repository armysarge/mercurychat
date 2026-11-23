#!/usr/bin/env bash
# The release .deb (M5 Task 12), inside mercurychat-app-build:jammy.
# Mounts: /src the worktree (ro, the UI already built in apps/desktop/dist),
#         /bundle the built-in modem for linux-amd64 (ro: mercury, rigctld,
#         BUILD.txt, MANIFEST.sha256), /hostreg the host crate cache (ro),
#         /build a volume, /tools a volume (tauri-cli, installed once), /out.
set -euo pipefail
rm -rf /build/src && mkdir -p /build/src
tar -C /src --exclude=./target --exclude=./node_modules --exclude=./apps/desktop/node_modules \
    --exclude=./lab --exclude=./evidence --exclude=./.superpowers -cf - . | tar -C /build/src -xf -
cd /build/src
test -f apps/desktop/dist/index.html || { echo "FAIL apps/desktop/dist missing: build the UI on the host first" >&2; exit 2; }
test -f /bundle/MANIFEST.sha256 || { echo "FAIL no built-in modem at /bundle" >&2; exit 2; }
export CARGO_TARGET_DIR=/build/target CARGO_HOME=/build/cargo-home
mkdir -p "$CARGO_HOME/registry"
[ -d "$CARGO_HOME/registry/cache" ] || cp -r /hostreg/cache /hostreg/index "$CARGO_HOME/registry/"
if ! command -v cargo-tauri >/dev/null; then
  cargo install tauri-cli --version 2.12.4 --locked --root /tools
fi

# The modem goes to /usr/lib/mercurychat, its manifest compiled into the app.
# A copy, so the programs can be given 0755 (the mount is read-only) and the data files 0644.
rm -rf /build/modem && cp -r /bundle /build/modem
chmod 0644 /build/modem/* && chmod 0755 /build/modem/mercury /build/modem/rigctld
files=""
for f in /build/modem/*; do
  files="$files\"/usr/lib/mercurychat/$(basename "$f")\":\"$f\","
done
cat > /build/release.json <<JSON
{"build":{"beforeBuildCommand":""},"bundle":{"linux":{"deb":{"files":{${files%,}}}}}}
JSON
cat /build/release.json
export MC_BUNDLE_MANIFEST=/build/modem/MANIFEST.sha256 CARGO_NET_OFFLINE=true
cargo tauri build --bundles deb --config /build/release.json

dir=$CARGO_TARGET_DIR/release
mkdir -p /out/linux-amd64
cp "$dir"/bundle/deb/*.deb /out/linux-amd64/
for d in /out/linux-amd64/*.deb; do
  echo "deb=$(basename "$d") sha256=$(sha256sum "$d" | cut -d' ' -f1) depends=$(dpkg-deb -f "$d" Depends)"
  dpkg-deb -c "$d" | awk '{print $1, $6}' | grep -E 'usr/(bin|lib/mercurychat)/'
done
