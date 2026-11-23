#!/usr/bin/env bash
# Test build-bundle.sh --latest without the network (M5 Task 1 Step 1b), inside
# mercurychat-bundle:jammy with the same mounts as build-bundle.sh plus /out.
#   1. a resolver returning the development pins builds a "latest" bundle with
#      the same versions and files as the pinned one;
#   2. a resolver returning a Mercury where patch 0001 can't apply stops the
#      build with "patch 0001 does not apply to Mercury <version>".
#   test-latest.sh <old mercury commit where 0001 does not apply>
set -uo pipefail
old=${1:?an old Mercury commit}
. /pins/hamlib-pin.env
. /pins/mercury-pin.env
fail() { echo "FAIL $*" >&2; exit 1; }

cat > /tmp/stub-pins.sh <<EOF
echo HAMLIB_VERSION=$HAMLIB_VERSION
echo HAMLIB_URL=/hamlib-src/hamlib-$HAMLIB_VERSION.tar.gz
echo MERCURY_REPO=/src
echo MERCURY_REF=$MERCURY_COMMIT
EOF
MC_RESOLVE=/tmp/stub-pins.sh bash /scripts/build-bundle.sh --latest linux-amd64 > /tmp/latest-1.log 2>&1 \
  || { tail -20 /tmp/latest-1.log >&2; fail "latest build with the pins failed"; }
b=/out/linux-amd64/BUILD.txt
grep -qx "mode=latest" "$b" || fail "BUILD.txt does not say mode=latest"
grep -qx "hamlib_version=$HAMLIB_VERSION" "$b" || fail "hamlib version differs"
grep -qx "mercury_commit=$MERCURY_COMMIT" "$b" || fail "mercury commit differs"
[ "$(awk '{print $2}' /out/linux-amd64/MANIFEST.sha256 | tr '\n' ' ')" = "BUILD.txt mercury rigctld " ] \
  || fail "file list differs: $(awk '{print $2}' /out/linux-amd64/MANIFEST.sha256 | tr '\n' ' ')"
echo "ok   --latest with the pins gives the same versions and files"

cat > /tmp/stub-old.sh <<EOF
echo HAMLIB_VERSION=$HAMLIB_VERSION
echo HAMLIB_URL=/hamlib-src/hamlib-$HAMLIB_VERSION.tar.gz
echo MERCURY_REPO=/src
echo MERCURY_REF=$old
EOF
rm -rf /out/stop && mkdir -p /out/stop
MC_RESOLVE=/tmp/stub-old.sh bash /scripts/build-bundle.sh --latest linux-amd64 > /tmp/latest-2.log 2>&1
rc=$?
[ $rc -ne 0 ] || fail "the build went on with a patch that does not apply"
grep -q "FAIL patch 0001 does not apply to Mercury" /tmp/latest-2.log || { cat /tmp/latest-2.log >&2; fail "no 'patch 0001 does not apply' message"; }
echo "ok   a Mercury where 0001 can't apply stops the build: $(grep 'FAIL patch' /tmp/latest-2.log)"
echo "PASS test-latest"
