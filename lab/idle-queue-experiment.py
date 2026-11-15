"""M0 experiment: does Mercury queue data-port bytes written while idle, and
does ABORT discard them? Uses station B (8400/8401) of the running lab."""
import socket, sys, time
host, base = "127.0.0.1", int(sys.argv[1]) if len(sys.argv) > 1 else 8400
c = socket.create_connection((host, base)); d = socket.create_connection((host, base + 1))
c.settimeout(0.3)
def lines(wait=1.5):
    out, buf, end = [], b"", time.time() + wait
    while time.time() < end:
        try:
            chunk = c.recv(4096)
            if not chunk: break
            buf += chunk
        except socket.timeout:
            pass
    return [l for l in buf.decode(errors="replace").replace("\r\n", "\r").split("\r") if l]
def cmd(s, wait=1.5):
    c.sendall(s.encode() + b"\r"); r = lines(wait); print(f"> {s:<12} < {r}"); return r
cmd("BUFFER")
d.sendall(b"STALE-BYTES-WRITTEN-WHILE-IDLE"); time.sleep(0.5)
print("wrote 30 bytes to the idle data port")
cmd("BUFFER")
cmd("ABORT", 3)
cmd("BUFFER")
cmd("DISCONNECT", 2)
cmd("BUFFER")
c.close(); d.close()
