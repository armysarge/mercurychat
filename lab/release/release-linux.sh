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
# Refreshed every run (-u copies only what is newer): a copy taken once goes stale when a crate is added.
cp -ru /hostreg/cache /hostreg/index "$CARGO_HOME/registry/"
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
cargo tauri build --bundles deb --config /build/release.json -- --no-default-features   # no simulator in a release (M5 §5.6)

# A release carries no simulator: its fake station must not be in the binary.
if grep -aq "SIM1 station notes" "$CARGO_TARGET_DIR/release/mercurychat"; then echo "release binary contains the simulator" >&2; exit 1; fi

dir=$CARGO_TARGET_DIR/release
mkdir -p /out/linux-amd64
# A pre-release must sort before its release: in a .deb, "0.5.0-beta.1" reads as
# version 0.5.0 revision beta.1, which dpkg puts AFTER 0.5.0, so the final release
# would never upgrade over the beta. Debian writes a pre-release with "~"
# (0.5.0~beta.1 < 0.5.0). Repack with that version, and name the file to match.
for d in "$dir"/bundle/deb/*.deb; do
  v=$(dpkg-deb -f "$d" Version)
  if [[ $v == *-* ]]; then
    dv=${v/-/\~}
    rm -rf /build/repack && dpkg-deb -R "$d" /build/repack
    sed -i "s/^Version: .*/Version: $dv/" /build/repack/DEBIAN/control
    dpkg-deb --root-owner-group -b /build/repack "/out/linux-amd64/$(basename "${d/$v/$dv}")" >/dev/null
    dpkg --compare-versions "$dv" lt "${dv%%~*}" || { echo "FAIL $dv does not sort before ${dv%%~*}" >&2; exit 1; }
    echo "deb version $v -> $dv (sorts before ${dv%%~*})"
  else
    cp "$d" /out/linux-amd64/
  fi
done
for d in /out/linux-amd64/*.deb; do
  echo "deb=$(basename "$d") sha256=$(sha256sum "$d" | cut -d' ' -f1) depends=$(dpkg-deb -f "$d" Depends)"
  dpkg-deb -c "$d" | awk '{print $1, $6}' | grep -E 'usr/(bin|lib/mercurychat)/'
done
