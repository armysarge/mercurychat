#!/usr/bin/env python3
"""Mercury's own view, seen by a second UI client (M3b.1 rig lab).

Usage: ws-snoop.py PORT [SECONDS]

Connects to Mercury's UI WebSocket (it needs -G), collects what Mercury
sends on connect for SECONDS (default 2), and prints one JSON object: the
last message of each type (radio_list, capture_dev_list, playback_dev_list,
audio_system, input_channel) plus the audio fields of the last status. It
only listens; it sends no command. rig_lab reads it to check what reached
Mercury independently of MercuryChat's own telemetry link.
"""
import base64, json, os, socket, struct, sys, time

port = int(sys.argv[1])
secs = float(sys.argv[2]) if len(sys.argv) > 2 else 2.0
s = socket.create_connection(("127.0.0.1", port), timeout=5)
key = base64.b64encode(os.urandom(16)).decode()
s.sendall((f"GET /websocket HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
           f"Upgrade: websocket\r\nConnection: Upgrade\r\n"
           f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
hdr = b""
while b"\r\n\r\n" not in hdr:
    hdr += s.recv(1)
if b" 101 " not in hdr.split(b"\r\n")[0]:
    sys.exit(f"WebSocket upgrade refused: {hdr[:80]!r}")

buf = b""
def take(n):
    global buf
    while len(buf) < n:
        d = s.recv(65536)
        if not d:
            raise ConnectionError
        buf += d
    out, buf = buf[:n], buf[n:]
    return out

seen = {}
end = time.monotonic() + secs
try:
    while time.monotonic() < end:
        s.settimeout(max(0.05, end - time.monotonic()))
        h = take(2)
        op, ln = h[0] & 0x0F, h[1] & 0x7F
        if ln == 126: ln = struct.unpack(">H", take(2))[0]
        elif ln == 127: ln = struct.unpack(">Q", take(8))[0]
        mask = take(4) if h[1] & 0x80 else None
        p = take(ln)
        if mask: p = bytes(c ^ mask[i % 4] for i, c in enumerate(p))
        if op != 1: continue
        try: m = json.loads(p)
        except ValueError: continue
        t = m.get("type")
        if t == "status":
            seen["status"] = {k: m[k] for k in ("audio_ok", "audio_error") if k in m}
        elif t:
            seen[t] = m
except (socket.timeout, ConnectionError, OSError):
    pass
print(json.dumps(seen))
