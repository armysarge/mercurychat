#!/usr/bin/env bash
# Dry-runs build-release.sh with stand-ins for docker, node, npm, cargo and the
# resolver, and checks what they are asked to do. Nothing is built: the stand-in
# cargo stops the run at the Windows installer. Run in Git Bash:
#   bash lab/release/test-build-release.sh
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
# A version no release has, so the output folder is this test's own.
version=0.0.0-test-build-release
out=E:/mc-lab-out/m5/release/$version
[ ! -e "$out" ] || { echo "FAIL $out already exists"; exit 1; }
trap 'rm -rf "$work" "$out"' EXIT
mkdir -p "$work/bin"
failed=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; sed 's/^/     | /' "$work/log" "$work/calls" 2>/dev/null; failed=1; }

stub() { printf '#!/usr/bin/env bash\n%s\n' "$2" > "$work/bin/$1"; chmod +x "$work/bin/$1"; }
stub node "case \$1 in -p) echo $version ;; esac"
stub npm ':'
# One line per argument, then a line with "end".
stub cargo "{ echo cargo; printf 'arg=%s\n' \"\$@\"; echo end; } >> '$work/calls'; exit 1"
stub docker "echo \"docker \$*\" >> '$work/calls'
for a in \"\$@\"; do case \$a in *:/out) o=\${a%:/out} ;; *:/resolved:ro) [ ! -f \"\${a%:/resolved:ro}/resolved.env\" ] || echo sources-present >> '$work/calls' ;; esac; target=\$a; done
mkdir -p \"\$o/\$target\" && echo mode=latest > \"\$o/\$target/BUILD.txt\" && : > \"\$o/\$target/MANIFEST.sha256\""
# resolve-release.sh <out dir> [--approve X]
stub resolver "echo \"resolver \$*\" >> '$work/calls'
[ -z \"\${STUB_REFUSE:-}\" ] || { echo 'FAIL not approved' >&2; exit 4; }
mkdir -p \"\$1\" && echo HAMLIB_VERSION=9.1 > \"\$1/resolved.env\""

run() {
  : > "$work/calls"
  PATH="$work/bin:$PATH" MC_RESOLVER=$work/bin/resolver bash "$here/build-release.sh" "$@" < /dev/null > "$work/log" 2>&1
}

# A refused resolve: no build, and an earlier release's output stays.
mkdir -p "$out" && echo keep > "$out/earlier"
STUB_REFUSE=1 run
if grep -q '^resolver ' "$work/calls" && ! grep -qE '^(docker|cargo)' "$work/calls"; then ok "a refused resolve builds nothing"
else bad "a refused resolve"; fi
[ -f "$out/earlier" ] && ok "and keeps the earlier output" || bad "the earlier output is gone"

run --approve "hamlib=9.1 mercury=0123456789ab"
resolvers=$(grep -c '^resolver ' "$work/calls")
mounts=$(grep '^docker ' "$work/calls" | grep -o -- '-v [^ ]*:/resolved:ro' | sort)
if [ "$resolvers" = 1 ]; then ok "the sources are resolved once"; else bad "resolver runs: $resolvers"; fi
if grep -q '^resolver .* --approve hamlib=9.1 mercury=0123456789ab$' "$work/calls"; then ok "--approve reaches the resolver"
else bad "--approve was not passed on"; fi
if [ "$(echo "$mounts" | grep -c resolved)" = 2 ] && [ "$(echo "$mounts" | sort -u | wc -l)" = 1 ]; then
  ok "both builds get the same checked sources"
else bad "resolved mounts: $mounts"; fi
[ "$(grep -c '^sources-present$' "$work/calls")" = 2 ] && ok "and they are there while each build runs" || bad "a build ran without resolved.env"

# The Windows installer: exactly these arguments, with the simulator left out.
got=$(sed -n '/^cargo$/,/^end$/p' "$work/calls" | sed -n 's/^arg=//p' | sed 's/^{.*}$/CONFIG/' | tr '\n' '|')
want="tauri|build|--bundles|nsis|--config|CONFIG|--|--no-default-features|"
[ "$got" = "$want" ] && ok "the installer build gets no stray arguments" || bad "cargo arguments: $got (wanted $want)"

# --pinned asks nobody and resolves nothing.
run --pinned
if ! grep -q '^resolver ' "$work/calls" && ! grep -q '/resolved:ro' "$work/calls" && grep -q '^docker ' "$work/calls"; then
  ok "--pinned does not resolve"
else bad "--pinned"; fi

[ $failed = 0 ] && echo "PASS test-build-release" || { echo "FAILED test-build-release"; exit 1; }
