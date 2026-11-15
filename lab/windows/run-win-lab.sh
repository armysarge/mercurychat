#!/usr/bin/env bash
# Windows L1 lab (docs/07 §2): two native mercury.exe instances over two
# independent VB-Audio cables, crossed, WASAPI, PTT none.
#
#   Station A: plays CABLE-A Input, captures CABLE-B Output   control 8300/8301, broadcast 8100
#   Station B: plays CABLE-B Input, captures CABLE-A Output   control 8400/8401, broadcast 8200
#
# Device ids come from `mercury.exe -x wasapi -z`; override with CA_IN/CA_OUT/CB_IN/CB_OUT.
# Run from the repo root in Git Bash. Stops both modems on exit.
set -euo pipefail

M=${M:-lab/bin/windows-x64/mercury.exe}
OUT=${OUT:-lab/run/win-l1}
CA_IN=${CA_IN:-'{0.0.0.00000000}.{1f68aa44-9ff8-4523-8429-02de487b53e0}'}   # CABLE-A Input
CA_OUT=${CA_OUT:-'{0.0.1.00000000}.{a3544a95-51f9-4cdc-aee9-359bd5b81053}'} # CABLE-A Output
CB_IN=${CB_IN:-'{0.0.0.00000000}.{964afa2f-cfaa-4fca-90ad-66241370063e}'}   # CABLE-B Input
CB_OUT=${CB_OUT:-'{0.0.1.00000000}.{22742773-d4c6-4f25-bdd3-686ec5e365cb}'} # CABLE-B Output
mkdir -p "$OUT/a" "$OUT/b"

# BIND=127.0.0.1 keeps every listener on loopback (needs a Mercury with the
# bind_address setting, upstream PR #565; see lab/mercury-patches).
for s in a b; do
  { [ -n "${BIND:-}" ] && printf '[main]\nbind_address = %s\n' "$BIND"; printf '[store]\nenabled = false\n'; } > "$OUT/$s/mercury.ini"
done

{
  echo "lab_started_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mercury_sha256=$(sha256sum "$M" | cut -d' ' -f1)"
  echo "audio=wasapi; A out CABLE-A Input / in CABLE-B Output; B out CABLE-B Input / in CABLE-A Output; PTT none"
} > "$OUT/lab-manifest.txt"

"$M" -C "$OUT/a/mercury.ini" -x wasapi -o "$CA_IN" -i "$CB_OUT" -p 8300 -b 8100 -P none -L "$OUT/a/mercury.log" > "$OUT/a/stdout.log" 2>&1 &
PA=$!
"$M" -C "$OUT/b/mercury.ini" -x wasapi -o "$CB_IN" -i "$CA_OUT" -p 8400 -b 8200 -P none -L "$OUT/b/mercury.log" > "$OUT/b/stdout.log" 2>&1 &
PB=$!
trap 'kill $PA $PB 2>/dev/null || true' EXIT

# Connect-only readiness probe (never write to an idle data port).
for port in 8300 8301 8400 8401; do
  for i in $(seq 1 100); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break; sleep 0.1; done
done
echo "WIN LAB READY: A=8300 B=8400"

# Sample both modems' CPU time and working set every 5 s.
( while sleep 5; do
    powershell -NoProfile -Command "Get-Process mercury -ErrorAction SilentlyContinue | ForEach-Object { '{0} {1} cpu_s={2:N1} ws_mb={3:N1}' -f (Get-Date).ToUniversalTime().ToString('HH:mm:ss'), \$_.Id, \$_.CPU, (\$_.WorkingSet64/1MB) }" 2>/dev/null
  done ) >> "$OUT/resources.log" &
RP=$!
trap 'kill $PA $PB $RP 2>/dev/null || true' EXIT

# RUN overrides the default scenario with another command (e.g. the broadcast probe).
if [ -n "${RUN:-}" ]; then
  eval "$RUN"
else
  ${CLIENT:-target/release/m0-client.exe} lab --a 127.0.0.1:8300 --b 127.0.0.1:8400 --work "$OUT/client" --file-bytes 4096 ${CLIENT_ARGS:-}
fi
