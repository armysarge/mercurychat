#!/usr/bin/env bash
# Listener exposure check (docs/02 §4, evidence L06). Starts one mercury.exe
# with the UI WebSocket on, records the address of every listening socket it
# owns, and tries to connect to each port through this PC's LAN address.
#
#   exposure-check.sh <mercury.exe> <out-dir> [bind_address]
#
# Audio is the null backend and PTT none: nothing is played or keyed.
set -uo pipefail
M=$1; OUT=$2; BIND=${3:-}
mkdir -p "$OUT"
{
  echo "[main]"
  [ -n "$BIND" ] && echo "bind_address = $BIND"
  echo "[store]"
  echo "enabled = false"
} > "$OUT/mercury.ini"

LAN=$(powershell -NoProfile -Command "(Get-NetIPAddress -AddressFamily IPv4 | Where-Object { \$_.IPAddress -notlike '127.*' -and \$_.IPAddress -notlike '169.254.*' -and \$_.InterfaceAlias -notlike 'vEthernet*' } | Select-Object -First 1).IPAddress" | tr -d '\r')

"$M" -C "$OUT/mercury.ini" -x null -p 8300 -b 8100 -G -U 10000 -W -P none -L "$OUT/mercury.log" > "$OUT/stdout.log" 2>&1 &
PID=$!
for i in $(seq 1 50); do (exec 3<>/dev/tcp/127.0.0.1/8300) 2>/dev/null && break; kill -0 $PID 2>/dev/null || break; sleep 0.2; done
sleep 1
WPID=$(ps | awk -v p=$PID '$1==p {print $4}')   # Git Bash ps: PID PPID PGID WINPID ...
{
  echo "bind_address=${BIND:-<unset>}"
  echo "lan_address=$LAN"
  if [ -z "$WPID" ] || ! kill -0 $PID 2>/dev/null; then
    echo "mercury_running=no"
    echo "stdout: $(tr -d '\033' < "$OUT/stdout.log" | sed 's/\[[0-9;]*m//g' | grep -i -E 'bind|error|invalid|fail' | head -3 | tr '\n' ' ')"
  else
    echo "mercury_running=yes"
    echo "listeners (netstat, pid $WPID):"
    netstat -ano | tr -d '\r' | awk -v p="$WPID" '$1=="TCP" && $4=="LISTENING" && $5==p {print "  " $2}' | sort
    for port in 8300 8301 8100 10000; do
      loop=refused; lan=refused
      timeout 2 bash -c "exec 3<>/dev/tcp/127.0.0.1/$port" 2>/dev/null && loop=open
      [ -n "$LAN" ] && timeout 2 bash -c "exec 3<>/dev/tcp/$LAN/$port" 2>/dev/null && lan=open
      echo "port $port: loopback=$loop lan=$lan"
    done
  fi
} > "$OUT/exposure.txt"
kill $PID 2>/dev/null; wait $PID 2>/dev/null
cat "$OUT/exposure.txt"
