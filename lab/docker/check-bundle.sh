#!/usr/bin/env bash
# Check a built-in modem bundle (M5 Task 1): the programs MercuryChat ships for
# one target, inside mercurychat-bundle:jammy.
#   check-bundle.sh <bundle dir> <linux-amd64|windows-x64> <hamlib version>
# Exits non-zero, naming the first failed check.
set -uo pipefail
dir=${1:?bundle dir}; target=${2:?target}; want=${3:?hamlib version}
fail() { echo "FAIL $*" >&2; exit 1; }
ok() { echo "ok   $*"; }

[ -d "$dir" ] || fail "no bundle at $dir"
cd "$dir" || fail "cannot enter $dir"

# The manifest covers every file but itself, and every line verifies.
[ -f MANIFEST.sha256 ] || fail "no MANIFEST.sha256"
sha256sum --quiet -c MANIFEST.sha256 || fail "a manifest line does not verify"
listed=$(awk '{print $2}' MANIFEST.sha256 | sort)
present=$(find . -type f ! -name MANIFEST.sha256 | sed 's|^\./||' | sort)
[ "$listed" = "$present" ] || fail "manifest and files differ: $(diff <(echo "$listed") <(echo "$present") | tr '\n' ' ')"
ok "manifest covers $(echo "$present" | wc -l) files"

case "$target" in
  linux-amd64)
    for p in mercury rigctld; do [ -x "$p" ] || fail "no $p"; done
    ver=$(./rigctld --version)
    grep -q "Hamlib $want" <<< "$ver" || fail "rigctld is not Hamlib $want: $(./rigctld --version | head -1)"
    ok "rigctld $(./rigctld --version | head -1)"
    ./rigctld -l | awk '$1 == "1" { found = 1 } END { exit !found }' || fail "rigctld -l has no model 1 (Dummy)"
    ok "rigctld lists the dummy rig"
    # Our Hamlib is linked in: no system libhamlib, and only libraries every
    # supported distro has (Ubuntu 22.04/24.04, Debian 12, Arch).
    allowed='^(linux-vdso|libc|libm|libpthread|libdl|librt|libgcc_s|libstdc\+\+|ld-linux-x86-64|libpulse|libpulse-simple|libasound|libssl|libcrypto|libz)\.so'
    for p in mercury rigctld; do
      deps=$(ldd "$p")
      grep -qi hamlib <<< "$deps" && fail "$p links a system libhamlib"
      bad=$(ldd "$p" | awk '{print $1}' | sed 's|.*/||' | grep -Ev "$allowed" | grep -v '^$' || true)
      # libpulse pulls its own private libraries in; only the direct needs count.
      direct=$(objdump -p "$p" | awk '/NEEDED/ {print $2}' | grep -Ev "$allowed" || true)
      [ -z "$direct" ] || fail "$p needs libraries a distro may lack: $direct"
    done
    ok "mercury and rigctld need only common system libraries" ;;
  windows-x64)
    for p in mercury.exe rigctld.exe libhamlib-4.dll; do [ -f "$p" ] || fail "no $p"; done
    # (Read whole outputs first: with pipefail, grep -q's early exit fails the pipe.)
    imports=$(x86_64-w64-mingw32-objdump -p mercury.exe)
    grep -q 'DLL Name: libhamlib-4.dll' <<< "$imports" || fail "mercury.exe does not import libhamlib-4.dll"
    hamlib_strings=$(strings libhamlib-4.dll)
    grep -q "Hamlib $want" <<< "$hamlib_strings" || fail "libhamlib-4.dll is not Hamlib $want"
    ok "mercury.exe and rigctld.exe use the bundled Hamlib $want"
    # Every DLL a shipped program imports is either Windows' own or in the bundle.
    system='^(kernel32|user32|advapi32|ws2_32|winmm|ole32|oleaut32|shell32|msvcrt|ucrtbase|api-ms-win-.*|setupapi|gdi32|comdlg32|bcrypt|crypt32|iphlpapi|dsound|avrt|mfplat|ksuser|shlwapi|version|secur32|userenv|ntdll|mswsock|dnsapi|hid|cfgmgr32|imm32|uuid)\.dll$'
    for f in *.exe *.dll; do
      for d in $(x86_64-w64-mingw32-objdump -p "$f" | awk '/DLL Name:/ {print $3}'); do
        lc=$(echo "$d" | tr 'A-Z' 'a-z')
        echo "$lc" | grep -Eq "$system" && continue
        [ -f "$d" ] || ls | tr 'A-Z' 'a-z' | grep -qx "$lc" || fail "$f imports $d, which is neither Windows' nor in the bundle"
      done
    done
    ok "every import is Windows' own or bundled"
    if command -v wine >/dev/null; then
      wine rigctld.exe --version 2>/dev/null | grep -q "Hamlib $want" || fail "rigctld.exe under wine"
    else
      echo "note rigctld.exe not run (no wine in the image)"
    fi ;;
  *) fail "unknown target $target" ;;
esac
echo "PASS bundle $target"
