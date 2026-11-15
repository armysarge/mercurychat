#!/usr/bin/env bash
# Run Mercury tests for a change in a Mercury working tree mounted at /src.
#
#   run-it.sh <go -run regex> [unit test ...]
#
# Builds two trees: /base (the branch's merge base with upstream mercuryv2,
# i.e. without the change) and /fix (the working tree as mounted, committed or
# not). Runs the named unit tests on /fix, then the Go integration test(s) on
# both, so a test that exercises the fault is seen to fail before and pass
# after. Results go to /out.
set -uo pipefail
RUN=$1; shift
UNITS=("$@")
OUT=${OUT:-/out}
mkdir -p "$OUT"
git config --global --add safe.directory '*'

base=$(git -C /src merge-base HEAD "$(git -C /src rev-parse --verify -q upstream/mercuryv2 || echo origin/mercuryv2)")
echo "base=$base" | tee "$OUT/summary.txt"

git clone -q /src /base && git -C /base checkout -q "$base"
git clone -q /src /fix && git -C /fix checkout -q HEAD
( cd /src && git diff HEAD ) | git -C /fix apply --whitespace=nowarn
( cd /src && git ls-files --others --exclude-standard -z ) | while IFS= read -r -d '' f; do
  mkdir -p "/fix/$(dirname "$f")"; cp "/src/$f" "/fix/$f"
done
# Integration tests the change adds or edits (committed on the branch or not)
# run against the base build too.
( cd /fix && { git diff --name-only "$base" HEAD; git status --porcelain | awk '{print $2}'; } | grep '^tests/integration/' | sort -u ) | while read -r f; do
  [ -f "/fix/$f" ] && cp "/fix/$f" "/base/$f"
done
git -C /fix diff --stat HEAD | tail -1 | tee -a "$OUT/summary.txt"

for tree in base fix; do
  echo "=== build $tree ===" | tee -a "$OUT/summary.txt"
  make -C "/$tree" -j"$(nproc)" > "$OUT/build-$tree.log" 2>&1 || { echo "build $tree FAILED" | tee -a "$OUT/summary.txt"; tail -30 "$OUT/build-$tree.log"; exit 2; }
done

for u in "${UNITS[@]}"; do
  ( cd /fix/tests && make -s "$u" > /dev/null 2>&1 && ./"$u" ) > "$OUT/unit-$u.log" 2>&1
  echo "unit $u: $(tail -2 "$OUT/unit-$u.log" | head -1)" | tee -a "$OUT/summary.txt"
done

for tree in base fix; do
  ( cd "/$tree/tests/integration" && MERCURY_BIN="/$tree/mercury" GOFLAGS=-mod=mod go test -count=1 -v -run "$RUN" ./... ) > "$OUT/it-$tree.log" 2>&1
  rc=$?
  echo "integration $tree: rc=$rc $(grep -E '^(--- |ok|FAIL|PASS)' "$OUT/it-$tree.log" | tr '\n' ' ')" | tee -a "$OUT/summary.txt"
done
