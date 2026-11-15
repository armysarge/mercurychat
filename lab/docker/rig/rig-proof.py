#!/usr/bin/env python3
"""Rig-control proof (docs/02 §5.1, D13), run inside run-rig-lab.sh.

Plays the MercuryChat role against two Mercury instances that key PTT through
their own Hamlib rigctld (dummy rig, model 1):
  * rigctl client  - read/set frequency and mode, watch PTT state;
  * TNC client     - MYCALL / LISTEN / CONNECT / data / DISCONNECT;
  * WebSocket      - Mercury frequency telemetry and live set_tx_gain.
Writes report.json and report.md into the output directory.
"""
import base64, json, os, signal, socket, struct, subprocess, sys, threading, time

OUT = sys.argv[1] if len(sys.argv) > 1 else "/out"
T0 = time.monotonic()
def now(): return round(time.monotonic() - T0, 3)

results, notes = [], []
def record(name, ok, detail, kind="check"):
    """kind='check' must pass; kind='finding' records observed Mercury/Hamlib behaviour."""
    results.append({"step": name, "pass": bool(ok), "kind": kind, "t": now(), "detail": detail})
    tag = ("PASS" if ok else "FAIL") if kind == "check" else "FIND"
    print(f"[{now():7.2f}] {tag} {name}: {detail}", flush=True)

# ---------------------------------------------------------------- rigctl ----
class Rig:
    """rigctld client using the '+' extended response protocol (ends in RPRT n)."""
    def __init__(self, port, timeout=3.0):
        self.port = port
        self.s = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        self.f = self.s.makefile("r", encoding="ascii", newline="\n")
    def cmd(self, c):
        self.s.sendall(("+" + c + "\n").encode())
        lines = []
        while True:
            line = self.f.readline()
            if not line:
                raise ConnectionError("rigctld closed")
            line = line.rstrip("\n")
            if line.startswith("RPRT"):
                return int(line.split()[1]), lines
            lines.append(line)
    def value(self, c, key):
        rc, lines = self.cmd(c)
        for l in lines:
            if l.startswith(key + ":"):
                return rc, l.split(":", 1)[1].strip()
        return rc, None
    def close(self):
        try: self.s.close()
        except OSError: pass

class PttWatch(threading.Thread):
    """Polls 't' every 50 ms on its own connection; records transitions and outages."""
    def __init__(self, name, port):
        super().__init__(daemon=True)
        self.name_, self.port = name, port
        self.events, self.state, self.stop = [], None, False
        self.errors, self.polls, self.slow = [], 0, []
        self.lock = threading.Lock()
    def run(self):
        rig = None
        while not self.stop:
            try:
                if rig is None:
                    rig = Rig(self.port, timeout=1.0)
                t1 = time.monotonic()
                rc, v = rig.value("t", "PTT")
                ms = (time.monotonic() - t1) * 1000
                self.polls += 1
                if ms > 200: self.slow.append((now(), round(ms)))
                # RPRT != 0 means "PTT state unavailable", never "unkeyed".
                st = int(v) if rc == 0 else f"unavailable(RPRT {rc})"
            except (OSError, ConnectionError, ValueError, TypeError) as e:
                if len(self.errors) < 50: self.errors.append((now(), type(e).__name__, str(e)[:80]))
                if rig: rig.close()
                rig, st = None, "unreachable"
            with self.lock:
                if st != self.state:
                    self.events.append((now(), st))
                    self.state = st
            time.sleep(0.05 if rig else 0.2)
    def keyed_times(self):
        with self.lock:
            return [t for t, s in self.events if s == 1]
    def wait_state(self, want, timeout):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.state == want: return True
            time.sleep(0.02)
        return False

# ------------------------------------------------------------------- TNC ----
class Tnc(threading.Thread):
    def __init__(self, name, port):
        super().__init__(daemon=True)
        self.name_ = name
        self.s = socket.create_connection(("127.0.0.1", port), timeout=5)
        self.s.settimeout(None)
        self.lines, self.lock = [], threading.Lock()
        self.start()
    def run(self):
        buf = b""
        while True:
            try: d = self.s.recv(4096)
            except OSError: return
            if not d: return
            buf += d
            while b"\r" in buf:
                line, buf = buf.split(b"\r", 1)
                line = line.decode("ascii", "replace").strip()
                if line:
                    with self.lock: self.lines.append((now(), line))
    def send(self, c):
        self.s.sendall((c + "\r").encode())
    def wait(self, pred, timeout, since=0.0):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            with self.lock:
                for t, l in self.lines:
                    if t >= since and pred(l): return t, l
            time.sleep(0.05)
        return None
    def events(self, prefix):
        with self.lock: return [(t, l) for t, l in self.lines if l.startswith(prefix)]

class DataSink(threading.Thread):
    def __init__(self, port):
        super().__init__(daemon=True)
        self.s = socket.create_connection(("127.0.0.1", port), timeout=5)
        self.s.settimeout(None)
        self.received = bytearray()
        self.start()
    def run(self):
        while True:
            try: d = self.s.recv(65536)
            except OSError: return
            if not d: return
            self.received += d

# ------------------------------------------------------------- WebSocket ----
class Ws(threading.Thread):
    def __init__(self, port):
        super().__init__(daemon=True)
        self.s = socket.create_connection(("127.0.0.1", port), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall((f"GET /websocket HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                        f"Upgrade: websocket\r\nConnection: Upgrade\r\n"
                        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        hdr = b""
        while b"\r\n\r\n" not in hdr:
            hdr += self.s.recv(1)
        if b" 101 " not in hdr.split(b"\r\n")[0]:
            raise RuntimeError(f"WebSocket upgrade refused: {hdr[:80]!r}")
        self.s.settimeout(None)
        self.status, self.other = None, []
        self.start()
    def _recv(self, n):
        b = b""
        while len(b) < n:
            d = self.s.recv(n - len(b))
            if not d: raise ConnectionError
            b += d
        return b
    def run(self):
        try:
            while True:
                h = self._recv(2)
                op, ln = h[0] & 0x0F, h[1] & 0x7F
                if ln == 126: ln = struct.unpack(">H", self._recv(2))[0]
                elif ln == 127: ln = struct.unpack(">Q", self._recv(8))[0]
                mask = self._recv(4) if h[1] & 0x80 else None
                p = self._recv(ln)
                if mask: p = bytes(c ^ mask[i % 4] for i, c in enumerate(p))
                if op != 1: continue
                try: m = json.loads(p)
                except ValueError: continue
                if m.get("type") == "status": self.status = (now(), m)
                else: self.other.append((now(), m))
        except (OSError, ConnectionError):
            return
    def command(self, command, value=""):
        p = json.dumps({"command": command, "value": value}).encode()
        mask = os.urandom(4)
        hdr = bytes([0x81, 0x80 | len(p)]) if len(p) < 126 else bytes([0x81, 0x80 | 126]) + struct.pack(">H", len(p))
        self.s.sendall(hdr + mask + bytes(c ^ mask[i % 4] for i, c in enumerate(p)))
    def wait_status(self, pred, timeout):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            st = self.status
            if st and pred(st[1]): return st
            time.sleep(0.1)
        return None

def pid_of(name):
    with open(f"{OUT}/{name}/rigctld.pid") as f: return int(f.read().strip())

def alive(pidfile):
    try:
        with open(pidfile) as f: os.kill(int(f.read().strip()), 0)
        return True
    except (OSError, ValueError):
        return False

# ================================================================== run ====
def main():
    watch = {"a": PttWatch("a", 4532), "b": PttWatch("b", 4632)}
    for w in watch.values(): w.start()
    app = Rig(4532)

    # 1. Read the rig through the shared owner.
    rc, f = app.value("f", "Frequency")
    rc2, m = app.value("m", "Mode")
    record("rig_read", rc == 0 and f == "7105000" and m == "PKTUSB", f"frequency={f} mode={m}")

    # 2. MercuryChat sets frequency and mode while Mercury also holds a rigctld session.
    rc, _ = app.cmd("F 7104000")
    rc_m, _ = app.cmd("M USB 2400")
    _, f = app.value("f", "Frequency"); _, m = app.value("m", "Mode")
    record("rig_set_idle", rc == 0 and rc_m == 0 and f == "7104000" and m == "USB",
           f"F 7104000 -> RPRT {rc}; M USB 2400 -> RPRT {rc_m}; read back {f} {m}")
    app.cmd("M PKTUSB 3000")

    # 3. Mercury's own (read-only) frequency telemetry follows the change.
    ws_a, ws_b = Ws(10000), Ws(10100)
    st = ws_a.wait_status(lambda s: s.get("radio_frequency_hz") == 7104000, 15)
    record("mercury_frequency_telemetry", st is not None,
           f"status radio_frequency_hz={st[1].get('radio_frequency_hz') if st else (ws_a.status[1].get('radio_frequency_hz') if ws_a.status else None)}"
           f" age_ms={st[1].get('radio_frequency_age_ms') if st else None}")

    # 4. Per-band TX level: live set_tx_gain, confirmed from status, then restored.
    ws_a.command("set_tx_gain", "-3.5")
    st = ws_a.wait_status(lambda s: abs(float(s.get("tx_gain_db", 99)) + 3.5) < 0.01, 5)
    ws_a.command("set_tx_gain", "0")
    st0 = ws_a.wait_status(lambda s: abs(float(s.get("tx_gain_db", 99))) < 0.01, 5)
    record("tx_gain_live", st is not None and st0 is not None,
           f"-3.5 dB confirmed={st is not None}; restored 0 dB confirmed={st0 is not None}")

    # 5. Session: every Mercury PTT is carried out on the rig through rigctld.
    a, b = Tnc("a", 8300), Tnc("b", 8400)
    da, db = DataSink(8301), DataSink(8401)
    for tnc, call, listen in ((a, "LABA", "OFF"), (b, "LABB", "ON")):
        tnc.send(f"MYCALL {call}")
        tnc.wait(lambda l: l.startswith("REGISTERED") or l == "OK", 5)
        for c in ("PUBLIC OFF", f"LISTEN {listen}", "BW2300"):
            tnc.send(c); time.sleep(0.2)
    t_call = now()
    a.send("CONNECT LABA LABB")
    ca = a.wait(lambda l: l.startswith("CONNECTED"), 120, t_call)
    cb = b.wait(lambda l: l.startswith("CONNECTED"), 5 if ca else 1, t_call)
    record("connect_via_rigctld_ptt", bool(ca and cb),
           f"A {ca[1] if ca else 'no CONNECTED'} at +{(ca[0]-t_call) if ca else 0:.1f}s; B {cb[1] if cb else 'none'}")
    if not ca:
        return finish(watch, a, b)

    # 6. Rig owner does not protect a transmission: a set while keyed is accepted.
    #    Queue data first so A has something to transmit.
    payload1 = os.urandom(3000)
    da.s.sendall(payload1)
    if watch["a"].wait_state(1, 60):
        rc, _ = app.cmd("F 7110000")
        keyed = watch["a"].state == 1
        record("rigctld_accepts_set_while_keyed", True,
               f"F 7110000 while A keyed={keyed} -> RPRT {rc}: rigctld does not refuse, so MercuryChat must (RIG-03)")
        notes.append("A frequency set issued while PTT was on was accepted by rigctld; refusal is app policy.")
        watch["a"].wait_state(0, 30)
        app.cmd("F 7104000")
    else:
        record("rigctld_accepts_set_while_keyed", False, "A never keyed within 60 s")

    # 7. Exact data through the session.
    end = time.monotonic() + 180
    while time.monotonic() < end and len(db.received) < len(payload1): time.sleep(0.2)
    record("data_3000_bytes", bytes(db.received[:3000]) == payload1, f"B received {len(db.received)} bytes")

    # 8. Kill rigctld A while A is keyed.
    payload2 = os.urandom(3000)
    da.s.sendall(payload2)
    kill_info = {}
    if watch["a"].wait_state(1, 60):
        pid = pid_of("a")
        t_kill = now()
        os.kill(pid, signal.SIGKILL)
        kill_info["t_kill"] = t_kill
        time.sleep(15)
        merc_alive = alive(f"{OUT}/a/pid")
        ptt_lines = [l for t, l in a.lines if t >= t_kill and l.startswith("PTT")]
        kill_info.update(mercury_alive=merc_alive, control_ptt_after_kill=ptt_lines[:20],
                         b_bytes_at_15s=len(db.received) - 3000)
        record("kill_rigctld_while_keyed", merc_alive,
               f"Mercury A alive={merc_alive}; control PTT lines after kill={len(ptt_lines)}; "
               f"B got {len(db.received) - 3000}/3000 of payload 2 within 15 s")
        # Restart rigctld A (dummy state resets) and see whether Mercury's Hamlib reconnects.
        with open(f"{OUT}/a/rigctld.log", "a") as lg:
            p = subprocess.Popen(["rigctld", "-m", "1", "-P", "RIG", "-T", "127.0.0.1", "-t", "4532"], stdout=lg, stderr=lg)
        with open(f"{OUT}/a/rigctld.pid", "w") as f: f.write(str(p.pid))
        t_restart = now()
        time.sleep(1.0)
        app.close(); app = Rig(4532)
        app.cmd("F 7104000"); app.cmd("M PKTUSB 3000")
        rc, _ = app.cmd("T 0")
        _, ptt = app.value("t", "PTT")
        kill_info.update(t_restart=t_restart, app_ptt_off_rprt=rc, ptt_after_restart=ptt)
        end = time.monotonic() + 150
        while time.monotonic() < end and len(db.received) < 6000: time.sleep(0.2)
        # Give Mercury something new to transmit after the restart.
        payload3 = os.urandom(1500)
        da.s.sendall(payload3)
        end = time.monotonic() + 120
        while time.monotonic() < end and len(db.received) < 7500: time.sleep(0.2)
        time.sleep(2)
        kill_info["payload3_ok"] = bytes(db.received[6000:7500]) == payload3
        keyed_after = [t for t in watch["a"].keyed_times() if t > t_restart]
        merc_keys_after = [t for t, l in a.lines if t > t_restart and l == "PTT ON"]
        kill_info.update(rig_keyed_after_restart=len(keyed_after),
                         mercury_ptt_on_after_restart=len(merc_keys_after),
                         b_bytes_total=len(db.received))
        record("ptt_off_after_reconnect", rc == 0 and ptt == "0", f"app T 0 -> RPRT {rc}; t={ptt}")
        kill_info["mercury_reported_ptt_on_without_rig"] = len(merc_keys_after) > 0 and not keyed_after
        record("mercury_hamlib_session_after_rigctld_restart", len(keyed_after) > 0,
               f"Mercury reported PTT ON {len(merc_keys_after)}x after restart; rig keyed {len(keyed_after)}x; "
               f"payload 3 still delivered={kill_info['payload3_ok']} (virtual audio does not need PTT)", kind="finding")
        record("data_after_rig_outage", bytes(db.received[3000:6000]) == payload2,
               f"B received {len(db.received) - 3000}/3000 of payload 2")
    else:
        record("kill_rigctld_while_keyed", False, "A never keyed within 60 s")

    # 9. Disconnect.
    t_d = now()
    a.send("DISCONNECT")
    da_ = a.wait(lambda l: l.startswith("DISCONNECTED"), 180, t_d)
    db_ = b.wait(lambda l: l.startswith("DISCONNECTED"), 30, t_d)
    time.sleep(3)
    record("disconnect", bool(da_ and db_) and watch["a"].state == 0 and watch["b"].state == 0,
           f"A +{(da_[0]-t_d) if da_ else -1:.1f}s, B +{(db_[0]-t_d) if db_ else -1:.1f}s; "
           f"final rig PTT A={watch['a'].state} B={watch['b'].state}")

    # 10. Recovery: restarting Mercury (what managed mode would do) reopens its rigctld session.
    with open(f"{OUT}/a/pid") as f: mpid = int(f.read().strip())
    os.kill(mpid, signal.SIGTERM)
    for _ in range(100):
        if not alive(f"{OUT}/a/pid"): break
        time.sleep(0.1)
    a.s.close(); da.s.close()
    with open(f"{OUT}/a/stdout-restart.log", "w") as lg:
        mp = subprocess.Popen(["/mercury/linux-amd64/mercury", "-C", f"{OUT}/a/mercury.ini", "-x", "pulse",
                               "-o", "a_to_b", "-i", "b_to_a.monitor", "-p", "8300", "-b", "8100",
                               "-G", "-U", "10000", "-W", "-R", "2", "-A", "127.0.0.1:4532", "-H", "2",
                               "-L", f"{OUT}/a/mercury-restart.log"], stdout=lg, stderr=lg)
    with open(f"{OUT}/a/pid", "w") as f: f.write(str(mp.pid))
    for port in (8300, 8301):
        for _ in range(100):
            try: socket.create_connection(("127.0.0.1", port), timeout=1).close(); break
            except OSError: time.sleep(0.1)
    time.sleep(1)
    a2 = Tnc("a", 8300); DataSink(8301)
    a2.send("MYCALL LABA"); a2.wait(lambda l: l.startswith("REGISTERED") or l == "OK", 5)
    for c in ("PUBLIC OFF", "LISTEN OFF", "BW2300"): a2.send(c); time.sleep(0.2)
    t_r = now()
    a2.send("CONNECT LABA LABB")
    c2 = a2.wait(lambda l: l.startswith("CONNECTED"), 120, t_r)
    keyed_r = [t for t in watch["a"].keyed_times() if t > t_r]
    rows = correlate(a2, watch["a"])
    record("mercury_restart_restores_keying", bool(c2) and bool(keyed_r) and all(h for _, h in rows),
           f"reconnected={bool(c2)}; rig A keyed {len(keyed_r)}x; {sum(h for _, h in rows)}/{len(rows)} Mercury PTT ON seen on rig")
    if c2:
        t_d = now(); a2.send("DISCONNECT")
        a2.wait(lambda l: l.startswith("DISCONNECTED"), 120, t_d); time.sleep(3)
    finish(watch, a, b, kill_info)

def correlate(tnc, w, until=None):
    """Each Mercury 'PTT ON' on the control port should show as t=1 on its rig within 0.5 s."""
    keyed = w.keyed_times()
    rows = []
    for t, l in tnc.events("PTT ON"):
        if until is not None and t >= until: continue
        hit = [k for k in keyed if -0.5 <= k - t <= 0.5]
        rows.append((t, bool(hit)))
    return rows

def finish(watch, a, b, kill_info=None):
    kill_info = kill_info or {}
    cut = kill_info.get("t_kill")
    for name, tnc in (("a", a), ("b", b)):
        rows = correlate(tnc, watch[name], cut if name == "a" else None)
        matched = sum(1 for _, h in rows if h)
        record(f"ptt_correlation_{name}", rows and matched == len(rows),
               f"{matched}/{len(rows)} Mercury PTT ON events seen on rig {name.upper()}"
               + (" (before the rigctld kill)" if name == "a" and cut else ""))
    for w in watch.values(): w.stop = True
    report = {
        "watch": {k: {"polls": w.polls, "errors": w.errors, "slow": w.slow[:50]} for k, w in watch.items()},
        "results": results, "notes": notes, "kill": kill_info,
        "rig_ptt_events": {k: w.events for k, w in watch.items()},
        "control_a": a.lines, "control_b": b.lines,
    }
    with open(f"{OUT}/report.json", "w") as f: json.dump(report, f, indent=1)
    ok = all(r["pass"] for r in results if r["kind"] == "check")
    with open(f"{OUT}/report.md", "w") as f:
        checks = [r for r in results if r["kind"] == "check"]
        f.write(f"# Rig-control lab: {'PASS' if ok else 'FAIL'} ({sum(r['pass'] for r in checks)}/{len(checks)} checks)\n\n")
        f.write("| t (s) | Step | Result | Detail |\n|---|---|---|---|\n")
        for r in results:
            res = ("PASS" if r["pass"] else "FAIL") if r["kind"] == "check" else "finding"
            f.write(f"| {r['t']} | {r['step']} | {res} | {r['detail']} |\n")
        if kill_info:
            f.write("\n## rigctld kill while keyed\n\n```\n" + json.dumps(kill_info, indent=1) + "\n```\n")
    print(f"RESULT {'PASS' if ok else 'FAIL'}", flush=True)
    sys.exit(0 if ok else 1)

if __name__ == "__main__":
    main()
