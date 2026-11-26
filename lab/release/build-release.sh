#!/usr/bin/env bash
# A MercuryChat release (M5 Task 12), on the Windows host with Docker:
#   1. the built-in modem: build-bundle.sh --latest (always the newest Mercury
#      and Hamlib releases; owner, 23 Nov), Linux and Windows, plus their source.
#      resolve-release.sh chooses them once and asks you first (M1);
#   2. the Windows NSIS installer (per-user, the modem in modem\);
#   3. the .deb (Ubuntu 22.04 build, the modem in /usr/lib/mercurychat);
#   4. the Arch package (the .deb's files, repackaged);
#   5. THIRD-PARTY.md at the bundle's versions, MercuryChat's source zip, and
#      SHA256SUMS, all in E:/mc-lab-out/m5/release/<version>/.
#
#   bash lab/release/build-release.sh               a release: --latest modem
#   bash lab/release/build-release.sh --approve "hamlib=V mercury=C"
#                                  the same, approved without the question
#   bash lab/release/build-release.sh --pinned      the lab pins instead
#   bash lab/release/build-release.sh --bundle DIR  reuse a modem already built in DIR
#                                                   (DIR/linux-amd64, DIR/windows-x64, DIR/source)
# Never writes to the Mercury fork (read-only mount; `git archive` only reads).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
cd "$repo"
# However it ends, the worktree keeps the licence list at the lab pins (the gate checks it is current).
trap 'node lab/release/third-party.mjs >/dev/null' EXIT
mode=latest; bundle=""; approve=()
case "${1:-}" in
  --pinned) mode=pinned ;;
  --bundle) mode=reuse; bundle=$(cd "$2" && pwd) ;;
  --approve) approve=(--approve "${2:?--approve needs a value}") ;;
  "") ;;
  *) echo "usage: build-release.sh [--pinned | --bundle DIR | --approve \"hamlib=V mercury=C\"]" >&2; exit 2 ;;
esac
version=$(node -p "require('./src-tauri/tauri.conf.json').version")
out=E:/mc-lab-out/m5/release/$version
FORK=${MC_MERCURY_FORK:-E:/Programming/mercury-fork}
HAMLIB_SRC=${MC_HAMLIB_SRC:-E:/mc-lab-out/m5/src}
win() { if command -v cygpath >/dev/null; then cygpath -m "$1"; else echo "$1"; fi; }

# ---- 0. the modem sources of a release: chosen and approved once, for
# both builds. Before anything is deleted, so a refusal changes nothing.
resolved=""
if [ $mode = latest ]; then
  resolved=$(mktemp -d)
  trap 'rm -rf "$resolved"; node lab/release/third-party.mjs >/dev/null' EXIT
  bash "${MC_RESOLVER:-$here/resolve-release.sh}" "$resolved" ${approve[@]+"${approve[@]}"}
fi

rm -rf "$out" && mkdir -p "$out"
echo "== MercuryChat $version release into $out (modem: $mode)"

# ---- 1. the built-in modem
if [ $mode != reuse ]; then
  bundle=$out/bundle && mkdir -p "$bundle"
  flag=""; [ $mode = latest ] && flag=--latest
  M=(-v "$(win "$repo/lab/docker"):/scripts:ro" -v "$(win "$repo/lab"):/pins:ro" -v "$(win "$repo/lab/mercury-patches"):/patches:ro"
     -v "$FORK:/src:ro" -v "$HAMLIB_SRC:/hamlib-src:ro" -v "$(win "$bundle"):/out")
  [ -z "$resolved" ] || M+=(-v "$(win "$resolved"):/resolved:ro")
  MSYS_NO_PATHCONV=1 docker run --rm "${M[@]}" mercurychat-bundle:jammy bash /scripts/build-bundle.sh $flag linux-amd64
  MSYS_NO_PATHCONV=1 docker run --rm "${M[@]}" mercurychat-lab:m0 bash /scripts/build-bundle.sh $flag windows-x64
fi
for t in linux-amd64 windows-x64; do
  test -f "$bundle/$t/MANIFEST.sha256" || { echo "FAIL no modem for $t in $bundle" >&2; exit 1; }
done
grep -h "_version=\|^mode=" "$bundle/linux-amd64/BUILD.txt"

# ---- 5a. THIRD-PARTY.md at the bundle's versions (also compiled into About → Licences)
MC_BUNDLE_BUILD="$bundle/linux-amd64/BUILD.txt" node lab/release/third-party.mjs
npm --prefix apps/desktop run build

# ---- 2. Windows NSIS installer
wb=$(win "$bundle/windows-x64")
MC_BUNDLE_MANIFEST="$wb/MANIFEST.sha256" cargo tauri build --bundles nsis \
  --config "{\"build\":{\"beforeBuildCommand\":\"\"},\"bundle\":{\"resources\":{\"$wb/\":\"modem/\"}}}" \
  -- --no-default-features   # no simulator in a release (M5 §5.6)
cp target/release/bundle/nsis/*.exe "$out/"

# ---- 3. the .deb
MSYS_NO_PATHCONV=1 docker run --rm -v "$(win "$repo"):/src:ro" -v "$(win "$bundle/linux-amd64"):/bundle:ro" \
  -v "$(win "$HOME/.cargo/registry"):/hostreg:ro" -v mc-release-jammy:/build -v mc-release-tools:/tools \
  -v "$(win "$out"):/out" mercurychat-app-build:jammy bash /src/lab/release/release-linux.sh
deb=$(cd "$out/linux-amd64" && ls *.deb | head -1)

# ---- 4. the Arch package
MSYS_NO_PATHCONV=1 docker run --rm -v "$(win "$out/linux-amd64"):/pkg:ro" -v "$(win "$repo/packaging/arch"):/packaging:ro" \
  -v "$(win "$repo/lab/release"):/s:ro" -v "$(win "$out"):/out" mercurychat-arch-build bash /s/release-arch.sh "$deb" "$version"

# ---- 5b. sources and checksums
mkdir -p "$out/source"
git archive --format=zip --prefix="mercurychat-$version/" -o "$out/source/mercurychat-$version-source.zip" HEAD
if [ -d "$bundle/source" ]; then
  cp "$bundle/source/"* "$out/source/"
else
  # A reused lab bundle without its source: the pinned Mercury and Hamlib.
  . lab/mercury-pin.env; . lab/hamlib-pin.env
  git -C "$FORK" archive --format=zip --prefix="mercury-${MERCURY_COMMIT:0:12}/" -o "$out/source/mercury-${MERCURY_COMMIT:0:12}-source.zip" "$MERCURY_COMMIT"
  for p in $MERCURY_PATCHES; do cp "lab/mercury-patches/$p" "$out/source/"; done
  cp "$HAMLIB_SRC/hamlib-$HAMLIB_VERSION.tar.gz" "$out/source/"
fi
cp THIRD-PARTY.md LICENSE "$out/"
cp "$bundle/linux-amd64/BUILD.txt" "$out/BUILD-linux-amd64.txt"; cp "$bundle/windows-x64/BUILD.txt" "$out/BUILD-windows-x64.txt"
( cd "$out" && find . -type f ! -name SHA256SUMS ! -path './bundle/*' | sort | xargs sha256sum > SHA256SUMS )
echo "== done"; cat "$out/SHA256SUMS"
