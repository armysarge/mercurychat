#!/usr/bin/env python3
"""M0 broadcast/CQ probe (docs/03 §6) against two live Mercury modems.

Attaches to both stations' control, data and broadcast (KISS) ports and
measures, over the crossed virtual audio:
  * CQFRAME delivery to a receive-only (LISTEN OFF) and a listening station;
  * wrapped KISS 0x00 messages on the listen mode, exact payload and max size;
  * what happens when the listen modes differ;
  * station messages (type 0x07 via KISS 0x03) on BEACONMODE MFSK / DATAC16;
  * a broadcast submitted during an ARQ session;
  * BUSY detection and MONITOR lines on the receiver.
PTT none, virtual cables only. Writes probe.json / probe.md to the output dir.
"""
import json, os, socket, struct, sys, threading, time

OUT = sys.argv[1] if len(sys.argv) > 1 else "."
T0 = time.monotonic()
def now(): return round(time.monotonic() - T0, 2)
results = []
def record(name, ok, detail, kind="check"):
    results.append({"step": name, "pass": bool(ok), "kind": kind, "t": now(), "detail": detail})
    tag = ("PASS" if ok else "FAIL") if kind == "check" else "INFO"
    print(f"[{now():7.2f}] {tag} {name}: {detail}", flush=True)

FEND, FESC, TFEND, TFESC = 0xC0, 0xDB, 0xDC, 0xDD
def kiss_encode(cmd, payload):
    out = bytearray([FEND, cmd])
    for b in payload:
        if b == FEND: out += bytes([FESC, TFEND])
        elif b == FESC: out += bytes([FESC, TFESC])
        else: out.append(b)
    out.append(FEND)
    return bytes(out)

class Control(threading.Thread):
    def __init__(self, port):
        super().__init__(daemon=True)
        self.s = socket.create_connection(("127.0.0.1", port), timeout=5); self.s.settimeout(None)
        self.lines, self.lock = [], threading.Lock(); self.start()
    def run(self):
        buf = b""
        while True:
            try: d = self.s.recv(4096)
            except OSError: return
            if not d: return
            buf += d
            while b"\r" in buf:
                l, buf = buf.split(b"\r", 1)
                l = l.decode("ascii", "replace").strip()
                if l:
                    with self.lock: self.lines.append((now(), l))
    def send(self, c): self.s.sendall((c + "\r").encode())
    def cmd(self, c, timeout=5):
        t = now(); self.send(c)
        r = self.wait(lambda l: l in ("OK", "WRONG", "BUSY") or l.startswith(c.split()[0] + " "), timeout, t)
        return r[1] if r else None
    def wait(self, pred, timeout, since=0.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            with self.lock:
                for t, l in self.lines:
                    if t >= since and pred(l): return t, l
            time.sleep(0.05)
        return None
    def since(self, t, prefix=""):
        with self.lock: return [(tt, l) for tt, l in self.lines if tt >= t and l.startswith(prefix)]

class Kiss(threading.Thread):
    """Broadcast port client: collects decoded KISS frames (cmd, payload)."""
    def __init__(self, port):
        super().__init__(daemon=True)
        self.s = socket.create_connection(("127.0.0.1", port), timeout=5); self.s.settimeout(None)
        self.frames, self.lock = [], threading.Lock(); self.start()
    def run(self):
        cur, esc, inside = bytearray(), False, False
        while True:
            try: d = self.s.recv(4096)
            except OSError: return
            if not d: return
            for b in d:
                if b == FEND:
                    if inside and cur:
                        with self.lock: self.frames.append((now(), cur[0], bytes(cur[1:])))
                    cur, esc, inside = bytearray(), False, True
                elif esc:
                    cur.append(FEND if b == TFEND else FESC if b == TFESC else b); esc = False
                elif b == FESC: esc = True
                else: cur.append(b)
    def send(self, cmd, payload): self.s.sendall(kiss_encode(cmd, payload))
    def wait(self, pred, timeout, since):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            with self.lock:
                for f in self.frames:
                    if f[0] >= since and pred(f): return f
            time.sleep(0.05)
        return None

def mcb1(call, grid, seq):
    c, g = call.encode(), grid.encode()
    return b"MCB1" + bytes([1, 1]) + struct.pack(">I", seq) + bytes([len(c)]) + c + bytes([len(g)]) + g + struct.pack(">H", 0x0001)

def tx_window(ctl, t, timeout):
    """Seconds between the first PTT ON and the last PTT OFF after t (None if never keyed)."""
    end = time.monotonic() + timeout
    on = ctl.wait(lambda l: l == "PTT ON", timeout, t)
    if not on: return None
    off = ctl.wait(lambda l: l == "PTT OFF", max(1, end - time.monotonic()), on[0])
    return round(off[0] - on[0], 2) if off else None

def settle(a, b, quiet=1.5, timeout=40):
    """Wait until neither station has keyed for `quiet` seconds."""
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        last = max([t for t, l in a.since(0, "PTT") + b.since(0, "PTT")] or [0])
        on = [l for _, l in (a.since(0, "PTT") + b.since(0, "PTT"))]
        if now() - last >= quiet and (not on or a.since(0, "PTT")[-1:] in ([], [(a.since(0, "PTT")[-1:] or [(0, "PTT OFF")])[0]])):
            return
        time.sleep(0.2)

def main():
    a, b = Control(8300), Control(8400)
    da = socket.create_connection(("127.0.0.1", 8301)); db = socket.create_connection(("127.0.0.1", 8401))
    ka, kb = Kiss(8100), Kiss(8200)
    for ctl, call in ((a, "LABA"), (b, "LABB")):
        ctl.cmd(f"MYCALL {call}"); ctl.wait(lambda l: l.startswith("REGISTERED"), 5)
        for c in ("PUBLIC OFF", "LISTEN OFF", "BW2300"): ctl.cmd(c)
    info = {"A": {q: a.cmd(q) for q in ("MODE", "BEACONMODE", "MONITOR", "BUSYDETECT")},
            "B": {q: b.cmd(q) for q in ("MODE", "BEACONMODE", "MONITOR", "BUSYDETECT")}}
    record("initial_state", True, json.dumps(info), "info")

    # 1. CQFRAME to a receive-only (LISTEN OFF) station, then to a listening one.
    for listen in ("OFF", "ON"):
        b.cmd(f"LISTEN {listen}"); time.sleep(0.5)
        t = now(); r = a.cmd("CQFRAME LABA 2300")
        got = b.wait(lambda l: l.startswith("CQFRAME"), 25, t)
        win = tx_window(a, t, 10)
        pend = [l for _, l in a.since(t) if l in ("PENDING", "CANCELPENDING")]
        record(f"cq_to_listen_{listen.lower()}", got is not None,
               f"A reply={r}; A keyed {win}s; A host events {pend}; B heard={got[1] if got else None} at +{(got[0]-t) if got else 0:.1f}s")
        time.sleep(3)
    b.cmd("LISTEN OFF")

    # 2. Wrapped KISS 0x00 message on the shared listen mode (exact payload).
    mode = (a.cmd("MODE") or "").split()
    for listen in ("OFF", "ON"):
        b.cmd(f"LISTEN {listen}"); time.sleep(0.5)
        p = mcb1("LABA", "JJ00", 7 if listen == "OFF" else 8)
        t = now(); ka.send(0x00, p)
        got = kb.wait(lambda f: f[1] == 0x00 and f[2] == p, 30, t)
        anyf = [(round(f[0]-t,1), f[1], len(f[2])) for f in kb.frames if f[0] >= t]
        win = tx_window(a, t, 15)
        record(f"wrapped_beacon_listen_{listen.lower()}", got is not None,
               f"listen mode {' '.join(mode[1:])}; {len(p)}-byte MCB1; A keyed {win}s; B got exact={got is not None} at +{(got[0]-t) if got else 0:.1f}s; B frames {anyf}")
        time.sleep(3)
    b.cmd("LISTEN OFF")

    # 3. Capacity: largest wrapped payload on this mode, and one byte more.
    try: frame = int(mode[1]) and None
    except (IndexError, ValueError): frame = None
    cap = {"DATAC3": 126, "DATAC1": 510, "DATAC4": 54, "DATAC15": 30, "MFSK": 98, "MFSK500": 98}.get(mode[2] if len(mode) > 2 else "", 126) - 3
    for n, expect_exact in ((cap, True), (cap + 1, False)):
        p = bytes((i * 7 + n) & 0xFF for i in range(n))
        t = now(); ka.send(0x00, p)
        got = kb.wait(lambda f: f[1] == 0x00, 30, t)
        exact = got is not None and got[2] == p
        record(f"wrapped_size_{n}", exact == expect_exact,
               f"sent {n} bytes; received {len(got[2]) if got else None} bytes; exact={exact}"
               + ("" if expect_exact else " (over capacity: expected truncation or drop, never silent success)"))
        time.sleep(3)

    # 4. Listen-mode mismatch: B on MFSK, A on its default.
    r = b.cmd("MODE 11")
    p = mcb1("LABA", "JJ00", 9)
    t = now(); ka.send(0x00, p)
    got = kb.wait(lambda f: True, 25, t)
    record("mode_mismatch_not_heard", got is None, f"B MODE 11 -> {r}; B received {'a frame' if got else 'nothing'} from A on {' '.join(mode[1:])}")
    b.cmd(f"MODE {mode[1] if len(mode) > 1 else 1}")
    time.sleep(2)

    # 5. Station messages (type 0x07 via KISS 0x03) on BEACONMODE MFSK and DATAC16.
    for bm, size in ((11, 98), (8, 14)):
        r = a.cmd(f"BEACONMODE {bm}")
        for listen in ("OFF", "ON"):
            b.cmd(f"LISTEN {listen}"); time.sleep(0.5)
            body = mcb1("LABA", "JJ00", 20 + bm)[: size - 1]
            p = bytes([0x07 << 5]) + body + bytes(size - 1 - len(body))
            t = now(); ka.send(0x03, p)
            got = kb.wait(lambda f: f[1] == 0x03 and f[2][:1] == p[:1], 45, t)
            win = tx_window(a, t, 40)
            record(f"station_msg_bm{bm}_listen_{listen.lower()}", got is not None,
                   f"BEACONMODE {bm} -> {r}; {size}-byte frame; A keyed {win}s; B got={len(got[2]) if got else None} bytes, intact={got is not None and got[2][:size] == p} at +{(got[0]-t) if got else 0:.1f}s",
                   "info")
            time.sleep(3)
    b.cmd("LISTEN ON")  # B must answer the call below
    a.cmd("BEACONMODE 8")

    # 6. Broadcast submitted during an ARQ session.
    t = now(); a.send("CONNECT LABA LABB")
    c = a.wait(lambda l: l.startswith("CONNECTED"), 90, t)
    if c:
        p = mcb1("LABA", "JJ00", 30)
        tb = now(); ka.send(0x00, p)
        during = kb.wait(lambda f: f[2] == p, 12, tb)
        td = now(); a.send("DISCONNECT")
        dd = a.wait(lambda l: l.startswith("DISCONNECTED"), 120, td)
        after = kb.wait(lambda f: f[2] == p, 40, tb)
        record("broadcast_during_session", True,
               f"connected at +{c[0]-t:.1f}s; beacon heard during session={during is not None}; "
               f"heard at all={after is not None}" + (f" at +{after[0]-tb:.1f}s after submit (disconnect at +{(dd[0]-tb) if dd else -1:.1f}s)" if after else ""),
               "info")
    else:
        record("broadcast_during_session", False, "no CONNECTED within 90 s")
    time.sleep(3)
    b.cmd("LISTEN OFF")

    # 7. BUSY detection and MONITOR lines on B while A sends a CQ.
    rb = b.cmd("BUSYDETECT ON"); rm = b.cmd("MONITOR ON"); time.sleep(3)
    t = now(); a.cmd("CQFRAME LABA 500")
    b.wait(lambda l: l.startswith("CQFRAME"), 25, t); time.sleep(4)
    ev = [l for _, l in b.since(t) if l.startswith(("BUSY", "MONITOR", "CQFRAME"))]
    record("busy_and_monitor", any(l.startswith("MONITOR") for l in ev),
           f"BUSYDETECT ON -> {rb}; MONITOR ON -> {rm}; B events {ev}")
    b.cmd("MONITOR OFF"); b.cmd("BUSYDETECT OFF")

    ok = all(r["pass"] for r in results if r["kind"] == "check")
    with open(os.path.join(OUT, "probe.json"), "w") as f:
        json.dump({"results": results, "control_a": a.lines, "control_b": b.lines,
                   "kiss_b": [(t, c, p.hex()) for t, c, p in kb.frames],
                   "kiss_a": [(t, c, p.hex()) for t, c, p in ka.frames]}, f, indent=1)
    with open(os.path.join(OUT, "probe.md"), "w") as f:
        checks = [r for r in results if r["kind"] == "check"]
        f.write(f"# Broadcast probe: {'PASS' if ok else 'FAIL'} ({sum(r['pass'] for r in checks)}/{len(checks)} checks)\n\n| t (s) | Step | Result | Detail |\n|---|---|---|---|\n")
        for r in results:
            res = ("PASS" if r["pass"] else "FAIL") if r["kind"] == "check" else "info"
            f.write(f"| {r['t']} | {r['step']} | {res} | {r['detail'].replace('|', '/')} |\n")
    print("RESULT", "PASS" if ok else "FAIL", flush=True)
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
