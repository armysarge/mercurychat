#!/usr/bin/env python3
"""A virtual serial cable for the M3b lab's check 9: two pseudo-terminals,
/tmp/ttyV0 and /tmp/ttyV1, with bytes relayed between them (what socat's
pty,pty does). Mercury C opens one end as its "serial" rig; Hamlib's
rigctlcom answers on the other as a TS-2000 backed by the dummy rig. No real
serial device is involved."""
import os, pty, select, tty

m0, s0 = pty.openpty()
m1, s1 = pty.openpty()
for s, link in ((s0, "/tmp/ttyV0"), (s1, "/tmp/ttyV1")):
    tty.setraw(s)
    os.symlink(os.ttyname(s), link)
while True:
    ready, _, _ = select.select([m0, m1], [], [])
    for fd in ready:
        try:
            data = os.read(fd, 4096)
        except OSError:
            data = b""
        if data:
            os.write(m1 if fd == m0 else m0, data)
