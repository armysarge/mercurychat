#!/usr/bin/env bash
# Choose and get approval for the modem sources of a release (audit M1; release
# builds always take the latest Mercury and Hamlib releases). Run once, on the
# build host, so the Linux and Windows builds use exactly the same thing.
#
#   resolve-release.sh <out dir> [--approve "hamlib=<version> mercury=<12 of the commit>"]
#
# Hamlib: the latest release's source tarball, which must contain that version.
# Mercury: the latest release's tag, resolved to a commit.
# Neither is signature-checked (owner, 26 Nov 2026: Mercury signs nothing, and
# Hamlib's signing key has expired), so nothing here proves that a new release is
# genuine. The checks are the record of what was approved before
# (approved-modem.txt: a version may not change) and your approval. No release
# means no build: a branch is never used.
# Writes <out dir>/resolved.env and the tarball only after approval.
#
# The MC_* variables below exist for test-resolve-release.sh.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
api=${MC_API_BASE:-https://api.github.com}
hamlib_dl=${MC_HAMLIB_DL:-https://github.com/Hamlib/Hamlib/releases/download}
mercury_repo=${MC_MERCURY_REPO:-https://github.com/Rhizomatica/mercury.git}
approved=${MC_APPROVED:-$here/approved-modem.txt}
fail() { echo "FAIL $*" >&2; exit 4; }

out=""; answer=""; have_answer=0
while [ $# -gt 0 ]; do
  case "$1" in
    --approve) [ $# -ge 2 ] || fail "--approve needs a value"; answer=$2; have_answer=1; shift 2 ;;
    --) shift ;;
    *) [ -z "$out" ] || fail "unexpected argument $1"; out=$1; shift ;;
  esac
done
[ -n "$out" ] || fail "usage: resolve-release.sh <out dir> [--approve \"hamlib=V mercury=C\"]"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
rm -f "$out/resolved.env"

# The tag of a repository's latest release. Tag names come from outside and are
# used in file names, URLs and a sourced file: only plain version names pass.
latest_tag() {
  curl -fsSL "$api/repos/$1/releases/latest" 2>/dev/null \
    | grep -o '"tag_name": *"[^"]*"' | head -n 1 | sed 's/^"tag_name": *"//; s/"$//'
}

# ---- Hamlib
hv=$(latest_tag Hamlib/Hamlib) || true
[ -n "$hv" ] || fail "no Hamlib release found"
[[ $hv =~ ^[0-9]+(\.[0-9]+){1,3}$ ]] || fail "unexpected Hamlib tag: $(printf %q "$hv")"
name=hamlib-$hv.tar.gz
curl -fsSL "$hamlib_dl/$hv/$name" -o "$tmp/$name" || fail "cannot download $name"
# An older tarball must not pass as the newest.
tar -tzf "$tmp/$name" > "$tmp/list" || fail "$name is not a readable tarball"
top=$(sed 's|^\./||; s|/.*||' "$tmp/list" | sort -u)
[ "$top" = "hamlib-$hv" ] || fail "$name does not contain hamlib-$hv (it has: $(echo "$top" | head -n 3 | tr '\n' ' '))"
sha=$(sha256sum "$tmp/$name" | cut -d' ' -f1)

# ---- Mercury
mt=$(latest_tag Rhizomatica/mercury) || true
[ -n "$mt" ] || fail "no Mercury release found (a release is never built from a branch)"
[[ $mt =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || fail "unexpected Mercury tag: $(printf %q "$mt")"
refs=$(git ls-remote "$mercury_repo" "refs/tags/$mt" "refs/tags/$mt^{}") || fail "cannot read Mercury's tags"
# An annotated tag also has a ^{} line, which names the commit.
commit=$(echo "$refs" | awk '$2 ~ /\^\{\}$/ {peeled=$1} $2 !~ /\^\{\}$/ {plain=$1} END {print peeled ? peeled : plain}')
[[ $commit =~ ^[0-9a-f]{40}$ ]] || fail "Mercury's latest release is '$mt', but there is no tag of that name"

# ---- what was approved before
recorded() { # name version -> the recorded value, if any
  [ -f "$approved" ] && awk -v n="$1" -v v="$2" '$1 == n && $2 == v {print $3}' "$approved" || true
}
old_sha=$(recorded hamlib "$hv")
old_commit=$(recorded mercury "$mt")
[ -z "$old_sha" ] || [ "$old_sha" = "$sha" ] \
  || fail "Hamlib $hv has changed since it was approved: SHA-256 $sha, approved $old_sha"
[ -z "$old_commit" ] || [ "$old_commit" = "$commit" ] \
  || fail "Mercury's tag $mt has moved since it was approved: now $commit, approved $old_commit"
state() { if [ -n "$1" ]; then echo "approved before"; else echo "NEW"; fi; }

# ---- approval
cat <<EOF
== modem sources for this release
Hamlib   $hv  [$(state "$old_sha")]
         SHA-256 $sha
Mercury  $mt  [$(state "$old_commit")]
         commit $commit
neither is signature-checked: for anything NEW, check on the project's own pages
that this release is one you expect
EOF
want="hamlib=$hv mercury=${commit:0:12}"
if [ $have_answer = 1 ]; then
  [ "$answer" = "$want" ] || fail "--approve \"$answer\" does not match what was found: \"$want\""
else
  [ -t 0 ] || fail "nobody to ask. After checking the above, run again with --approve \"$want\""
  reply=""
  read -r -p "Build a release from these? Type yes: " reply || true
  [ "$reply" = yes ] || fail "not approved"
fi

mkdir -p "$out"
cp "$tmp/$name" "$out/$name"
cat > "$out/resolved.env" <<EOF
HAMLIB_VERSION=$hv
HAMLIB_SHA256=$sha
MERCURY_REPO=$mercury_repo
MERCURY_TAG=$mt
MERCURY_REF=$commit
EOF
touch "$approved"
[ -n "$old_sha" ] || echo "hamlib $hv $sha" >> "$approved"
[ -n "$old_commit" ] || echo "mercury $mt $commit" >> "$approved"
if [ -z "$old_sha" ] || [ -z "$old_commit" ]; then echo "recorded in $approved: commit it with the release"; fi
