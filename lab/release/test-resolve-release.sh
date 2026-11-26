#!/usr/bin/env bash
# Tests resolve-release.sh (audit M1) with no network: a fake GitHub API and
# Hamlib download in a temporary folder, and a local Mercury repository.
# Needs bash, curl, git and tar (Git Bash has them).
#   bash lab/release/test-resolve-release.sh
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
url() { if command -v cygpath >/dev/null; then echo "file:///$(cygpath -m "$1")"; else echo "file://$1"; fi; }
failed=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; sed 's/^/     | /' "$work/log"; failed=1; }

# A Mercury repository with a lightweight and an annotated tag.
git init -q "$work/mercury"
g() { git -C "$work/mercury" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false -c tag.gpgsign=false "$@"; }
g commit -q --allow-empty -m one && g tag v1.0 && one=$(g rev-parse HEAD)
g commit -q --allow-empty -m two && g tag -a v2.0 -m two && two=$(g rev-parse HEAD)
g commit -q --allow-empty -m head

# fixture <hamlib tag> [directory inside the tarball]
fixture() {
  rm -rf "$work/dl" "$work/src" && mkdir -p "$work/dl/$1" "$work/src/${2:-hamlib-$1}"
  echo "source $1" > "$work/src/${2:-hamlib-$1}/README"
  tar -C "$work/src" -czf "$work/dl/$1/hamlib-$1.tar.gz" "${2:-hamlib-$1}"
}
api() { # repository, tag ("" = the repository has no release)
  mkdir -p "$work/api/repos/$1/releases"
  rm -f "$work/api/repos/$1/releases/latest"
  [ -z "$2" ] || printf '{"url":"x","tag_name":"%s","name":"n"}\n' "$2" > "$work/api/repos/$1/releases/latest"
}
reset() {
  fixture 9.1
  api Hamlib/Hamlib 9.1
  api Rhizomatica/mercury v2.0
  : > "$work/approved.txt"
  rm -rf "$work/out"
}
run() { # extra arguments to resolve-release.sh; stdin is not a terminal
  MC_API_BASE=$(url "$work/api") MC_HAMLIB_DL=$(url "$work/dl") MC_MERCURY_REPO=$work/mercury \
  MC_APPROVED=$work/approved.txt \
    bash "$here/resolve-release.sh" "$work/out" "$@" < /dev/null > "$work/log" 2>&1
}
yes_to() { echo "hamlib=${1:-9.1} mercury=${2:-${two:0:12}}"; }
# refused <description> <text the output must contain> [arguments]
refused() {
  local what=$1 want=$2; shift 2
  [ $# -gt 0 ] || set -- --approve "$(yes_to)"
  if run "$@"; then bad "$what: it was accepted"
  elif ! grep -qF -- "$want" "$work/log"; then bad "$what: no '$want' in the output"
  elif [ -e "$work/out/resolved.env" ]; then bad "$what: resolved.env was written"
  else ok "$what"; fi
}

reset
if run --approve "$(yes_to)"; then
  sha=$(sha256sum "$work/dl/9.1/hamlib-9.1.tar.gz" | cut -d' ' -f1)
  e=$work/out/resolved.env
  if grep -qx "HAMLIB_VERSION=9.1" "$e" && grep -qx "HAMLIB_SHA256=$sha" "$e" \
     && grep -qx "MERCURY_TAG=v2.0" "$e" && grep -qx "MERCURY_REF=$two" "$e" \
     && cmp -s "$work/dl/9.1/hamlib-9.1.tar.gz" "$work/out/hamlib-9.1.tar.gz" \
     && grep -qx "hamlib 9.1 $sha" "$work/approved.txt" && grep -qx "mercury v2.0 $two" "$work/approved.txt"; then
    ok "an approved release resolves (annotated tag: the commit, not the tag object) and is recorded"
  else bad "wrong result: $(cat "$e" "$work/approved.txt")"; fi
else bad "a good release was refused"; fi
grep -q "neither is signature-checked" "$work/log" && ok "the question says that no signature is checked"   || bad "no word about signatures at the approval"

# Run again with the record in place: same answer, no duplicate lines.
rm -rf "$work/out"
if run --approve "$(yes_to)" && [ "$(wc -l < "$work/approved.txt")" -eq 2 ]; then ok "approving again records nothing twice"
else bad "second approval: $(cat "$work/approved.txt")"; fi

reset; api Rhizomatica/mercury v1.0
if run --approve "$(yes_to 9.1 "${one:0:12}")" && grep -qx "MERCURY_REF=$one" "$work/out/resolved.env"; then
  ok "a lightweight tag resolves to its commit"
else bad "lightweight tag"; fi

reset; rm "$work/dl/9.1/hamlib-9.1.tar.gz"
refused "a release without its source tarball" "cannot download"

reset; echo "not a tarball" > "$work/dl/9.1/hamlib-9.1.tar.gz"
refused "a download that is not a tarball" "not a readable tarball"

reset; fixture 9.1 hamlib-9.0
refused "an older tarball under a newer name" "does not contain hamlib-9.1"

reset; api Rhizomatica/mercury ""
refused "Mercury with no release: no branch instead" "no Mercury release"

reset; api Hamlib/Hamlib ""
refused "no Hamlib release" "no Hamlib release"

reset; api Rhizomatica/mercury mercuryv2
refused "a Mercury release that names a branch, not a tag" "no tag"

reset; api Rhizomatica/mercury "v2.0;touch\${IFS}$work/pwned"
refused "a tag name with shell code" "unexpected Mercury tag"
reset; api Hamlib/Hamlib "\$(touch $work/pwned)"
refused "a Hamlib tag name with shell code" "unexpected Hamlib tag"
[ ! -e "$work/pwned" ] && ok "no tag name was executed" || bad "a tag name was executed"

reset; echo "mercury v2.0 $one" > "$work/approved.txt"
refused "a Mercury tag moved since it was approved" "moved"

reset; echo "hamlib 9.1 $(printf '0%.0s' {1..64})" > "$work/approved.txt"
refused "a Hamlib release replaced since it was approved" "changed"

reset
refused "nobody to ask and no approval" "--approve" --
reset
refused "approval of a different version" "does not match" --approve "$(yes_to 9.0)"
reset
refused "approval of a different commit" "does not match" --approve "$(yes_to 9.1 "${one:0:12}")"
[ ! -s "$work/approved.txt" ] && ok "a refusal records nothing" || bad "a refusal wrote the record"

[ $failed = 0 ] && echo "PASS test-resolve-release" || { echo "FAILED test-resolve-release"; exit 1; }
