#!/usr/bin/env bash
# Resolve the newest Hamlib release and Mercury release for a release build
# (M5 design §3: release builds always take the latest). Prints shell
# assignments for build-bundle.sh to eval:
#   HAMLIB_VERSION, HAMLIB_URL, MERCURY_REPO, MERCURY_REF
# Mercury: its latest GitHub release tag, or the head of mercuryv2 if it has none.
set -euo pipefail
tag() { curl -fsSL "https://api.github.com/repos/$1/releases/latest" 2>/dev/null \
          | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1; }
hv=$(tag Hamlib/Hamlib)
[ -n "$hv" ] || { echo "echo 'FAIL cannot resolve the latest Hamlib release' >&2; exit 4"; exit 0; }
mt=$(tag Rhizomatica/mercury || true)
echo "HAMLIB_VERSION=$hv"
echo "HAMLIB_URL=https://github.com/Hamlib/Hamlib/releases/download/$hv/hamlib-$hv.tar.gz"
echo "MERCURY_REPO=https://github.com/Rhizomatica/mercury.git"
echo "MERCURY_REF=${mt:-mercuryv2}"
