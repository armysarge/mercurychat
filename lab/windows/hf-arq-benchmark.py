"""Measure raw ARQ between two configured, off-air modem instances."""

import argparse
from collections import Counter
from difflib import SequenceMatcher
import hashlib
import json
from pathlib import Path
import random
import socket
import struct
import threading
import time


class Station:
    def __init__(self, stack, port, log, name):
        self.log = log
        self.name = name
        self.control = stack.enter_context(socket.create_connection(("127.0.0.1", port), 5))
        self.control.settimeout(None)
        self.data = stack.enter_context(socket.create_connection(("127.0.0.1", port + 1), 5))
        self.events = []
        self.condition = threading.Condition()
        self.reader = threading.Thread(target=self.read_control)
        self.reader.start()
        stack.callback(self.close_control)

    def close_control(self):
        try:
            self.control.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.reader.join(2)

    def read_control(self):
        with self.control.makefile("rb") as stream:
            pending = b""
            while True:
                try:
                    chunk = stream.read1(8192)
                except OSError as exc:
                    self.log(f"{self.name} control socket closed: {exc}")
                    break
                if not chunk:
                    break
                pending += chunk
                lines = pending.split(b"\r")
                pending = lines.pop()
                for raw in lines:
                    line = raw.decode("ascii", errors="replace").strip()
                    if line:
                        ts = time.monotonic()
                        with self.condition:
                            self.events.append((ts, line))
                            self.condition.notify_all()
                        self.log(f"{ts:.6f} {self.name} < {line}")

    def send(self, command):
        sent = time.monotonic()
        self.log(f"{sent:.6f} {self.name} > {command}")
        self.control.sendall((command + "\r").encode("ascii"))
        return sent

    def wait(self, predicate, since, timeout):
        deadline = since + timeout
        with self.condition:
            while True:
                for ts, line in self.events:
                    if since <= ts <= deadline and predicate(line):
                        return ts, line
                remain = deadline - time.monotonic()
                if remain <= 0:
                    raise TimeoutError(f"{self.name} control response")
                self.condition.wait(remain)

    def configure(self, call):
        for cmd in (f"MYCALL {call}", "BW2300", "LISTEN ON"):
            sent = self.send(cmd)
            _, result = self.wait(lambda line: line in ("OK", "WRONG"), sent, 8)
            if result != "OK":
                raise RuntimeError(f"{self.name} refused {cmd}")


RECORD_BYTES = 128


def frame_payload(payload):
    marker = hashlib.sha256(payload).digest()[:8]
    records = [payload[start:start + RECORD_BYTES] for start in range(0, len(payload), RECORD_BYTES)]
    wire = b"".join(marker + struct.pack("!I", index) + data for index, data in enumerate(records))
    return marker, records, wire


def compare_record(expected, actual, counts):
    if expected == actual:
        counts["matching_bytes"] += len(expected)
        return
    removed = Counter()
    inserted = []
    for op, a, b, c, d in SequenceMatcher(None, expected, actual, autojunk=False).get_opcodes():
        if op == "equal":
            counts["matching_bytes"] += b - a
        elif op == "replace":
            overlap = min(b - a, d - c)
            counts["corrupted_bytes"] += overlap
            if a + overlap < b:
                removed[expected[a + overlap:b]] += 1
            if c + overlap < d:
                inserted.append((b, actual[c + overlap:d]))
        elif op == "delete":
            removed[expected[a:b]] += 1
        else:
            inserted.append((a, actual[c:d]))
    for position, data in inserted:
        if removed[data]:
            removed[data] -= 1
            counts["reordered_bytes"] += len(data)
            counts["matching_bytes"] += len(data)
        elif data in (expected[position:position + len(data)], expected[max(0, position - len(data)):position]):
            counts["duplicated_bytes"] += len(data)
        else:
            counts["extra_bytes"] += len(data)
    counts["missing_bytes"] += sum(len(data) * copies for data, copies in removed.items())


def inspect_payload(marker, records, received):
    counts = dict.fromkeys(("matching_bytes", "missing_bytes", "corrupted_bytes", "duplicated_bytes",
                            "reordered_bytes", "extra_bytes", "unframed_wire_bytes"), 0)
    header_bytes = len(marker) + 4
    seen = set()
    highest_index = -1
    start = received.find(marker)
    counts["unframed_wire_bytes"] = len(received) if start < 0 else start
    while start >= 0:
        next_start = received.find(marker, start + len(marker))
        end = len(received) if next_start < 0 else next_start
        record = received[start:end]
        start = next_start
        if len(record) < header_bytes:
            counts["unframed_wire_bytes"] += len(record)
            continue
        index = struct.unpack_from("!I", record, len(marker))[0]
        actual = record[header_bytes:]
        if index >= len(records):
            counts["unframed_wire_bytes"] += len(record)
        elif index in seen:
            counts["duplicated_bytes"] += len(actual)
        else:
            seen.add(index)
            if index < highest_index:
                counts["reordered_bytes"] += min(len(actual), len(records[index]))
            highest_index = max(highest_index, index)
            compare_record(records[index], actual, counts)
    counts["missing_bytes"] += sum(len(data) for index, data in enumerate(records) if index not in seen)
    return counts


def trial(sender, receiver, from_call, to_call, payload, connect_timeout, timeout, result,
          integrity_grace=None, alignment=None, artifacts=None):
    if integrity_grace is not None:
        marker, records, wire = frame_payload(payload)
    else:
        wire = payload
    if artifacts is not None:
        (artifacts / "expected.bin").write_bytes(wire)
    result["wire_bytes_sent"] = len(wire)
    result["sha256_wire_sent"] = hashlib.sha256(wire).hexdigest()
    if alignment is not None:
        port = int((alignment / "ready").read_text(encoding="ascii"))
        with socket.create_connection(("127.0.0.1", port), 5) as sync:
            if sync.recv(1) != b"\x01":
                raise RuntimeError("Audio bridge did not start the impairment timeline")
            time.sleep(0.002)
    start = sender.send(f"CONNECT {from_call} {to_call}")
    result["started_monotonic"] = start
    origin, _ = sender.wait(lambda line: line.startswith("CONNECTED "), start, connect_timeout)
    remote, _ = receiver.wait(lambda line: line.startswith("CONNECTED "), start, connect_timeout)
    connected = max(origin, remote)
    result.update(connected=True, setup_seconds=connected - start, phase="transfer")
    received = bytearray()
    first_byte = None
    completion = None
    sending = time.monotonic()
    result["sending_monotonic"] = sending
    deadline = sending + timeout
    sender.data.settimeout(timeout)
    try:
        sender.data.sendall(wire)
    except OSError as exc:
        result["transfer_error"] = str(exc)
    # Cap retained data if a malfunction duplicates the stream indefinitely.
    limit = len(wire) if integrity_grace is None else len(wire) * 4 + 65536
    while len(received) < limit:
        remain = deadline - time.monotonic()
        if remain <= 0:
            break
        receiver.data.settimeout(remain)
        try:
            chunk = receiver.data.recv(min(65536, limit - len(received)))
        except TimeoutError:
            break
        except OSError as exc:
            result["transfer_error"] = str(exc)
            break
        if not chunk:
            break
        if first_byte is None:
            first_byte = time.monotonic()
        received.extend(chunk)
        if completion is None and len(received) >= len(wire) and received.startswith(wire):
            completion = time.monotonic()
            if integrity_grace is not None:
                deadline = completion + integrity_grace
    observed = time.monotonic()
    verified = received == wire
    ended = completion if verified else observed
    result.update({
        "finished_monotonic": observed,
        "first_byte_seconds": None if first_byte is None else first_byte - sending,
        "transfer_seconds": ended - sending,
        "connect_to_complete_seconds": ended - start,
        "received_bytes": len(received),
        "sha256_received": hashlib.sha256(received).hexdigest(),
        "verified": verified,
        "goodput_bps": len(payload) * 8 / (ended - sending) if verified else None,
    })
    if integrity_grace is not None:
        result.update(inspect_payload(marker, records, received))
        result["wire_bytes_received"] = len(received)
        result["observation_limit_reached"] = len(received) == limit
    if artifacts is not None:
        (artifacts / "received.bin").write_bytes(received)
    if verified:
        try:
            stop = sender.send("DISCONNECT")
            for station in (sender, receiver):
                station.wait(lambda line: line == "DISCONNECTED", stop, 25)
        except (OSError, TimeoutError) as exc:
            result["teardown_error"] = str(exc)


def main():
    from contextlib import ExitStack

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ports", nargs=2, type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--channel", required=True)
    parser.add_argument("--modem", required=True)
    parser.add_argument("--calls", nargs=2, default=("ZS1AAA", "ZS2BBB"))
    parser.add_argument("--bytes", type=int, default=8192)
    parser.add_argument("--timeout", type=float, default=90)
    parser.add_argument("--connect-timeout", type=float)
    parser.add_argument("--payload-seed", type=int, default=4242)
    parser.add_argument("--integrity-grace", type=float, help="Use numbered records and drain for this many seconds")
    parser.add_argument("--alignment", type=Path, help="Start the impairment timeline before CONNECT")
    args = parser.parse_args()
    if args.bytes < 1 or args.timeout <= 0 or (args.connect_timeout is not None and args.connect_timeout <= 0):
        parser.error("Payload size and timeouts must be positive")
    if args.integrity_grace is not None and args.integrity_grace <= 0:
        parser.error("--integrity-grace must be positive")
    payload = random.Random(args.payload_seed).randbytes(args.bytes)
    connect_timeout = args.connect_timeout if args.connect_timeout is not None else args.timeout
    result = {
        "modem": args.modem, "channel": args.channel, "bandwidth_hz": 2300,
        "utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "sent_bytes": len(payload), "sha256_sent": hashlib.sha256(payload).hexdigest(),
        "payload_seed": args.payload_seed, "integrity_grace_seconds": args.integrity_grace,
        "connect_timeout_seconds": connect_timeout, "transfer_timeout_seconds": args.timeout,
        "connected": False, "phase": "configuration",
    }
    args.output.mkdir(parents=True, exist_ok=False)
    with (args.output / "control.log").open("w", encoding="utf-8", buffering=1) as log:
        with ExitStack() as stack:
            stations = []
            try:
                for name, port in zip(("a", "b"), args.ports):
                    stations.append(Station(stack, port, lambda line: print(line, file=log), name))
                for station, call in zip(stations, args.calls):
                    station.configure(call)
                result["phase"] = "connect"
                trial(*stations, *args.calls, payload, connect_timeout, args.timeout, result,
                      args.integrity_grace, args.alignment,
                      artifacts=args.output if args.integrity_grace is not None else None)
            except (OSError, RuntimeError, ValueError) as exc:
                result.update(verified=False, error=str(exc), finished_monotonic=time.monotonic())
                if args.integrity_grace is not None and result["phase"] == "connect" and "started_monotonic" in result:
                    marker, records, _ = frame_payload(payload)
                    result.update(inspect_payload(marker, records, b""), wire_bytes_received=0)
                    (args.output / "received.bin").touch()
            finally:
                for station in stations:
                    try:
                        station.send("ABORT")
                    except OSError:
                        pass
    report = json.dumps(result, indent=2)
    (args.output / "result.json").write_text(report + "\n", encoding="utf-8")
    print(report)
    return 0 if result.get("verified") else 1


if __name__ == "__main__":
    raise SystemExit(main())
