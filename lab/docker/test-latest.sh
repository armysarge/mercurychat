#!/usr/bin/env bash
# Test build-bundle.sh --latest without the network (M5 Task 1 Step 1b), inside
# mercurychat-bundle:jammy with the same mounts as build-bundle.sh plus /out.
# --latest builds only what resolve-release.sh resolved and had approved:
#   1. without that result, with a tarball that differs from it, or with a
#      branch instead of a commit, the build stops;
#   2. a result naming the development pins builds a "latest" bundle with the
#      same versions and files as the pinned one;
#   3. a result naming a Mercury where patch 0001 can't apply stops the build
#      with "patch 0001 does not apply to Mercury <version>".
#   test-latest.sh <old mercury commit (40 hex) where 0001 does not apply>
set -uo pipefail
old=${1:?an old Mercury commit}
. /pins/hamlib-pin.env
. /pins/mercury-pin.env
fail() { echo "FAIL $*" >&2; exit 1; }

# resolved <dir> <mercury ref> [sha256]: what resolve-release.sh would leave in <dir>
resolved() {
  rm -rf "$1" && mkdir -p "$1" && cp "/hamlib-src/hamlib-$HAMLIB_VERSION.tar.gz" "$1/"
  printf '%s\n' "HAMLIB_VERSION=$HAMLIB_VERSION" "HAMLIB_SHA256=${3:-$HAMLIB_SHA256}" \
    "MERCURY_REPO=/src" "MERCURY_TAG=test" "MERCURY_REF=$2" > "$1/resolved.env"
}
# refused <resolved dir> <text> <description>: the build stops before building
refused() {
  MC_RESOLVED=$1 bash /scripts/build-bundle.sh --latest linux-amd64 > /tmp/refused.log 2>&1 \
    && fail "$3: the build went on"
  grep -qF -- "$2" /tmp/refused.log || { tail -20 /tmp/refused.log >&2; fail "$3: no '$2'"; }
  grep -q "^=== " /tmp/refused.log && fail "$3: it started building"
  echo "ok   $3 stops the build"
}
refused /tmp/nothing "resolve-release.sh" "no approved sources"
resolved /tmp/wrong "$MERCURY_COMMIT" "$(printf '0%.0s' {1..64})"
refused /tmp/wrong "SHA-256 differs" "a Hamlib tarball that differs from the approved one"
resolved /tmp/branch HEAD
refused /tmp/branch "not a full commit" "a branch or HEAD instead of a commit"

resolved /tmp/pins "$MERCURY_COMMIT"
MC_RESOLVED=/tmp/pins bash /scripts/build-bundle.sh --latest linux-amd64 > /tmp/latest-1.log 2>&1 \
  || { tail -20 /tmp/latest-1.log >&2; fail "latest build with the pins failed"; }
b=/out/linux-amd64/BUILD.txt
grep -qx "mode=latest" "$b" || fail "BUILD.txt does not say mode=latest"
grep -qx "hamlib_version=$HAMLIB_VERSION" "$b" || fail "hamlib version differs"
grep -qx "mercury_commit=$MERCURY_COMMIT" "$b" || fail "mercury commit differs"
[ "$(awk '{print $2}' /out/linux-amd64/MANIFEST.sha256 | tr '\n' ' ')" = "BUILD.txt mercury rigctld " ] \
  || fail "file list differs: $(awk '{print $2}' /out/linux-amd64/MANIFEST.sha256 | tr '\n' ' ')"
echo "ok   --latest with the pins gives the same versions and files"

resolved /tmp/old "$old"
rm -rf /out/stop && mkdir -p /out/stop
MC_RESOLVED=/tmp/old bash /scripts/build-bundle.sh --latest linux-amd64 > /tmp/latest-2.log 2>&1
rc=$?
[ $rc -ne 0 ] || fail "the build went on with a patch that does not apply"
grep -q "FAIL patch 0001 does not apply to Mercury" /tmp/latest-2.log || { cat /tmp/latest-2.log >&2; fail "no 'patch 0001 does not apply' message"; }
echo "ok   a Mercury where 0001 can't apply stops the build: $(grep 'FAIL patch' /tmp/latest-2.log)"
echo "PASS test-latest"
