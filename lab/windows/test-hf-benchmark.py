"""Offline regressions for the payload and channel measurements."""

from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import random
import subprocess
from tempfile import TemporaryDirectory
import time
import unittest

import numpy as np


def load(stem):
    spec = spec_from_file_location(stem, Path(__file__).with_name(stem + ".py"))
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


arq = load("hf-arq-benchmark")
bridge = load("hf-ionos-bridge")
suite = load("hf-benchmark-suite")


class IntegrityTests(unittest.TestCase):
    def setUp(self):
        self.payload = random.Random(4242).randbytes(256)
        self.marker, self.records, self.wire = arq.frame_payload(self.payload)

    def test_byte_and_record_faults(self):
        wire = self.wire
        cut = 44
        size = 12 + arq.RECORD_BYTES
        cases = [
            (wire, "matching_bytes", len(self.payload)),
            (b"", "missing_bytes", len(self.payload)),
            (wire[:cut] + wire[cut + 1:], "missing_bytes", 1),
            (wire[:cut] + wire[cut:cut + 1] + wire[cut:], "duplicated_bytes", 1),
            (wire[:cut] + bytes([wire[cut] ^ 16]) + wire[cut + 1:], "corrupted_bytes", 1),
            (wire[:cut] + wire[cut:cut + 2][::-1] + wire[cut + 2:], "reordered_bytes", 1),
            (wire + wire[:size], "duplicated_bytes", arq.RECORD_BYTES),
            (wire[size:] + wire[:size], "reordered_bytes", arq.RECORD_BYTES),
            (wire[size:], "missing_bytes", arq.RECORD_BYTES),
            (wire[1:], "unframed_wire_bytes", size - 1),
        ]
        for received, metric, value in cases:
            with self.subTest(metric=metric, value=value):
                self.assertEqual(arq.inspect_payload(self.marker, self.records, received)[metric], value)

    def test_receive_checks_data_beyond_expected_wire_length(self):
        class FakeStation:
            def __init__(self, chunks):
                self.chunks = iter(chunks)
                self.data = self

            def send(self, command):
                return time.monotonic()

            def wait(self, predicate, since, timeout):
                return time.monotonic(), ""

            def settimeout(self, timeout):
                pass

            def sendall(self, payload):
                pass

            def recv(self, length):
                return next(self.chunks, b"")

        size = 12 + arq.RECORD_BYTES
        result = {}
        arq.trial(FakeStation([]), FakeStation([self.wire, self.wire[:size]]),
                  "A", "B", self.payload, 1, 1, result, integrity_grace=0.1)
        self.assertFalse(result["verified"])
        self.assertEqual(result["duplicated_bytes"], arq.RECORD_BYTES)


class ChannelTests(unittest.TestCase):
    def test_impulse_replay_is_seeded_and_calibrated(self):
        def samples(seed):
            impulse = bridge.ImpulseNoise(seed, 0, -12)
            return np.concatenate([impulse.block(index) for index in range(impulse.period_blocks)])

        wave = samples(4243)
        np.testing.assert_array_equal(wave, samples(4243))
        self.assertFalse(np.array_equal(wave, samples(4244)))
        self.assertFalse(np.array_equal(wave[:, 0], wave[:, 1]))
        expected = bridge.REFERENCE_PEAK / np.sqrt(2) * bridge.IONOS_OUTPUT_GAIN * 10 ** (-12 / 20)
        np.testing.assert_allclose(np.sqrt(np.mean(wave ** 2, axis=0)), expected)
        self.assertTrue(np.count_nonzero(wave[:, 0]) <= bridge.ImpulseNoise.pulse_blocks * bridge.BLOCK)

    def test_ionos_replay_has_identical_pcm_for_same_seed(self):
        engine = suite.SOURCES / "ionos_pipe_48k_gain.exe"
        if not engine.is_file():
            self.skipTest("IONOS pipe is not built")
        signal = (5000 * np.sin(2 * np.pi * 1500 * np.arange(bridge.BLOCK) / bridge.RATE)).astype("<i2").tobytes()
        pcm = (b"\x01\x00" + signal + bytes(len(signal))) * 50
        profiles = {}
        for profile in ("WGN", "MPP"):
            with self.subTest(profile=profile):
                def render(seed):
                    command = [str(engine), "--mode", profile, "--paths", "2", "--block", str(bridge.BLOCK),
                               "--snr", "0", "--seed", str(seed), "--output-gain", "0.25"]
                    return subprocess.run(command, input=pcm, capture_output=True, check=True).stdout

                wave = render(4243)
                self.assertEqual(len(wave), 50 * 4 * bridge.BLOCK)
                self.assertEqual(wave, render(4243))
                self.assertNotEqual(wave, render(4244))
                profiles[profile] = wave
        self.assertNotEqual(profiles["WGN"], profiles["MPP"])


class SummaryTests(unittest.TestCase):
    def test_retry_units_and_absence_of_payload_telemetry(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            tx = root / "station-0"
            tx.mkdir()
            (tx / "VARAHF.log").write_text("Connecting to TEST-2... 1/15\nConnecting to TEST-2... 2/15\n")
            vara = suite.retransmission_stats(root, "VARA", False)
            self.assertEqual(vara["connection_retries"], 1)
            self.assertIsNone(vara["retransmissions"])
            (tx / "mercury.log").write_text(
                "[arq-fsm] state=CALLING ev=TX_STARTED\n" * 2
                + "HF_BENCH_RETRANSMISSIONS unit=pieces\n"
                + "HF_BENCH_RETRANSMIT count=2\nHF_BENCH_RETRANSMIT count=3\n")
            mercury = suite.retransmission_stats(root, "Mercury", False)
            self.assertEqual(mercury["connection_retries"], 1)
            self.assertEqual((mercury["retransmissions"], mercury["retransmission_unit"]), (5, "pieces"))

    def test_invalid_slots_are_excluded_and_valid_timeouts_are_failures(self):
        def row(label, seed, valid, verified):
            return {
                "variant": label, "seed": seed, "profile": "MPP", "snr_db": 0, "direction": "A-to-B",
                "calibration": False, "valid": valid, "connected": True, "verified": verified,
                "goodput_bps": 64 if verified else None, "setup_seconds": 10, "first_byte_seconds": 12,
                "completion_seconds": 27 if verified else None, "matching_bytes": 128 if verified else 32,
                "sender_airtime_seconds": 15,
            }

        plan = [(("MPP", 0), seed, False) for seed in (4243, 4244, 4245)]
        attempts = [row("baseline", 4243, True, True), row("candidate", 4243, True, False),
                    row("baseline", 4244, True, True), row("candidate", 4244, False, False)]
        results, pairs = suite.summarize(attempts, ["baseline", "candidate"], plan)
        self.assertEqual([row["planned_slots"] for row in results], [3, 3])
        self.assertEqual([row["valid_slots"] for row in results], [2, 1])
        self.assertEqual([row["matched_trials"] for row in results], [1, 1])
        self.assertEqual([row["delivery_success_rate"] for row in results], [1.0, 0.0])
        self.assertEqual([row["median_matching_bytes"] for row in results], [128, 32])
        self.assertEqual(len(pairs), 1)
        self.assertEqual(pairs[0]["matching_bytes_b_minus_a"], -96)


if __name__ == "__main__":
    unittest.main()
