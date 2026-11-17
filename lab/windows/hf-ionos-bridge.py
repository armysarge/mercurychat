"""Connect the three VB-Audio cables to the Ion HF port of ARSFI IONOS."""

import argparse
from contextlib import ExitStack
import json
from pathlib import Path
import queue
import socket
import subprocess
import threading
import time

import numpy as np


RATE = 48000
BLOCK = 960
REFERENCE_PEAK = 0.5
IONOS_OUTPUT_GAIN = 0.5


class ImpulseNoise:
    """One randomly placed, band-limited noise pulse per 400 ms per direction."""

    period_blocks = 20
    pulse_blocks = 2

    def __init__(self, seed, snr_db, rx_gain_db):
        self.generators = [np.random.default_rng(key) for key in (seed, seed ^ 0x5555)]
        size = self.pulse_blocks * BLOCK
        self.band = (np.fft.rfftfreq(size, 1 / RATE) >= 300) & (np.fft.rfftfreq(size, 1 / RATE) <= 3000)
        self.window = np.hanning(size)
        self.rms = REFERENCE_PEAK / np.sqrt(2) * 10 ** ((rx_gain_db - snr_db) / 20) * IONOS_OUTPUT_GAIN
        self.period = np.zeros((self.period_blocks * BLOCK, 2))

    def block(self, index):
        offset = index % self.period_blocks
        if offset == 0:
            self.period.fill(0)
            for channel, rng in enumerate(self.generators):
                spectrum = np.fft.rfft(rng.standard_normal(len(self.window)))
                spectrum[~self.band] = 0
                pulse = np.fft.irfft(spectrum) * self.window
                pulse *= self.rms * np.sqrt(len(self.period) / np.dot(pulse, pulse))
                start = rng.integers(0, len(self.period) - len(pulse) + 1)
                self.period[start:start + len(pulse), channel] = pulse
        return self.period[offset * BLOCK:(offset + 1) * BLOCK]


def endpoint(sd, prefix, direction):
    channel_key = f"max_{direction}_channels"
    wasapi = next(api for api in sd.query_hostapis() if api["name"] == "Windows WASAPI")
    found = [idx for idx in wasapi["devices"] if sd.query_devices(idx)["name"].startswith(prefix)
             and sd.query_devices(idx)[channel_key] >= 2]
    if len(found) != 1:
        raise RuntimeError(f"Expected one WASAPI endpoint for {prefix}, found {found}")
    return found[0]


def main():
    import sounddevice as sd

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=("WGN", "IMPULSE", "MPG", "MPM", "MPP", "MPD"), required=True)
    parser.add_argument("--seed", type=int, default=4242)
    parser.add_argument("--snr", type=float)
    parser.add_argument("--peaks", nargs=2, type=float)
    parser.add_argument("--loopback", type=Path)
    parser.add_argument("--pids", type=int, nargs=2)
    parser.add_argument("--rx-gain-db", type=float, default=0)
    parser.add_argument("--align", action="store_true")
    parser.add_argument("--normalize", action="store_true")
    args = parser.parse_args()
    if args.snr is not None and args.peaks is None:
        parser.error("--snr requires calibrated TX peaks via --peaks")
    if not -30 <= args.rx_gain_db <= 0:
        parser.error("--rx-gain-db must be between -30 and 0")
    if args.normalize and (args.peaks is None or any(not 0 < peak <= 1 for peak in args.peaks)):
        parser.error("--normalize requires calibrated TX peaks in (0, 1]")
    if args.mode == "IMPULSE" and (args.snr is None or not (args.normalize and args.align)):
        parser.error("IMPULSE requires --snr, --normalize and --align")
    mode = "WGN" if args.mode == "IMPULSE" else args.mode
    impulse = ImpulseNoise(args.seed, args.snr, args.rx_gain_db) if args.mode == "IMPULSE" else None
    tx_gain = [REFERENCE_PEAK / peak for peak in args.peaks] if args.normalize else [1.0, 1.0]
    command = [str(args.engine.resolve()), "--mode", mode, "--paths", "2",
               "--block", str(BLOCK), "--seed", str(args.seed),
               "--output-gain", str(10 ** (args.rx_gain_db / 20))]
    if args.snr is not None and impulse is None:
        a, b = [REFERENCE_PEAK] * 2 if args.normalize else args.peaks
        command += ["--snr", str(args.snr), "--peak-a", str(a), "--peak-b", str(b)]

    in_queues = [queue.Queue(maxsize=8), queue.Queue(maxsize=8)]
    playback = [queue.Queue(maxsize=8) for _ in (args.pids or [None])]
    armed = threading.Event()
    peaks = np.zeros(2)
    rx_peaks = np.zeros(2)
    counts = dict.fromkeys(("sample_errors", "input_overflow", "output_underrun", "blocks",
                            "impairment_blocks", "impulse_clipped_samples", "normalization_clipped_samples"), 0)
    fault_times = []
    impairment_started = None
    name_a, name_b, name_rx = "CABLE-A Output (", "CABLE-B Output (", "CABLE Input (VB-Audio Virtual"
    settings = dict(samplerate=RATE, channels=2, dtype="int16", blocksize=BLOCK,
                    extra_settings=sd.WasapiSettings(auto_convert=True), latency="low")

    def mark_fault(name, index):
        if armed.is_set():
            counts[name] += 1
            if len(fault_times) < 1000:
                fault_times.append([time.monotonic(), name, index])

    def enqueue(index, samples):
        try:
            in_queues[index].put_nowait(samples[:, 0].copy())
        except queue.Full:
            mark_fault("input_overflow", index)

    def capture(index):
        def callback(samples, frames, _time, status):
            if frames != BLOCK or status:
                mark_fault("sample_errors", index)
                return
            enqueue(index, samples)
        return callback

    def render(index):
        def callback(samples, frames, _time, status):
            if frames != BLOCK or status:
                mark_fault("sample_errors", index)
                samples.fill(0)
                return
            try:
                samples[:] = playback[index].get_nowait()
            except queue.Empty:
                samples.fill(0)
                mark_fault("output_underrun", index)
        return callback

    def read_loopback(index, pipe):
        while len(raw := pipe.read(BLOCK * 4)) == BLOCK * 4:
            enqueue(index, np.frombuffer(raw, dtype="<i2").reshape(-1, 2))

    args.output.mkdir(parents=True, exist_ok=True)
    stop = args.output / "stop"
    silence = np.zeros((BLOCK, 2), dtype="<i2")
    with (args.output / "ionos.log").open("wb") as log:
        with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log) as engine:
            try:
                with ExitStack() as streams:
                    sync_server = None
                    sync_client = None
                    if args.align:
                        sync_server = streams.enter_context(socket.socket())
                        sync_server.bind(("127.0.0.1", 0))
                        sync_server.listen(1)
                        sync_server.setblocking(False)
                    if args.pids:
                        outputs = [endpoint(sd, name, "output") for name in ("CABLE-B Input (", "CABLE-A Input (")]
                        for index, (pid, device) in enumerate(zip(args.pids, outputs)):
                            streams.enter_context(sd.OutputStream(device=device, callback=render(index), **settings))
                            capture_log = streams.enter_context((args.output / f"capture-{index}.log").open("wb"))
                            proc = streams.enter_context(subprocess.Popen(
                                [str(args.loopback), str(pid)], stdout=subprocess.PIPE, stderr=capture_log))
                            thread = threading.Thread(target=read_loopback, args=(index, proc.stdout))
                            thread.start()
                            streams.callback(thread.join, 2)
                            streams.callback(proc.terminate)
                    else:
                        inputs = [endpoint(sd, name, "input") for name in (name_a, name_b)]
                        output = endpoint(sd, name_rx, "output")
                        streams.enter_context(sd.OutputStream(device=output, callback=render(0), **settings))
                        for index, device in enumerate(inputs):
                            streams.enter_context(sd.InputStream(device=device, callback=capture(index), **settings))
                    while not stop.exists():
                        samples = [audio.get(timeout=3) for audio in in_queues]
                        level = np.array([np.max(np.abs(audio.astype("int32"))) / 32768 for audio in samples])
                        peaks = np.maximum(peaks, level)
                        if sync_server is not None and armed.is_set() and impairment_started is None:
                            try:
                                sync_client = streams.enter_context(sync_server.accept()[0])
                            except BlockingIOError:
                                pass
                        rx = silence
                        if not args.align or impairment_started is not None or sync_client is not None:
                            if args.normalize:
                                scaled = [np.rint(audio.astype(float) * gain) for audio, gain in zip(samples, tx_gain)]
                                counts["normalization_clipped_samples"] += sum(
                                    int(np.count_nonzero((audio < -32768) | (audio > 32767))) for audio in scaled
                                )
                                samples = [np.clip(audio, -32768, 32767).astype("<i2") for audio in scaled]
                            # Keying flags collect burst stats. The simulator processes every sample.
                            engine.stdin.write(bytes(int(value > 0.015) for value in level)
                                               + b"".join(audio.astype("<i2", copy=False).tobytes() for audio in samples))
                            engine.stdin.flush()
                            response = engine.stdout.read(BLOCK * 4)
                            if len(response) != BLOCK * 4:
                                raise RuntimeError("IONOS engine returned a short audio block")
                            rx = np.frombuffer(response, dtype="<i2").reshape(2, BLOCK).T.copy()
                            if impulse is not None:
                                mixed = np.rint(rx + 32768 * impulse.block(counts["impairment_blocks"]))
                                counts["impulse_clipped_samples"] += int(np.count_nonzero((mixed < -32768) | (mixed > 32767)))
                                rx = np.clip(mixed, -32768, 32767).astype("<i2")
                            if impairment_started is None:
                                impairment_started = time.monotonic()
                                if sync_client is not None:
                                    sync_client.sendall(b"\x01")
                                    sync_client.close()
                            counts["impairment_blocks"] += 1
                        rx_peaks = np.maximum(rx_peaks, np.max(np.abs(rx.astype("int32")), axis=0) / 32768)
                        if args.pids:
                            for index, audio in enumerate(playback):
                                audio.put(rx[:, index:index+1], timeout=2)
                        else:
                            playback[0].put(rx, timeout=2)
                        counts["blocks"] += 1
                        if counts["blocks"] == 30:
                            armed.set()
                            if sync_server is None:
                                (args.output / "ready").touch()
                            else:
                                ready = args.output / "ready.tmp"
                                ready.write_text(str(sync_server.getsockname()[1]), encoding="ascii")
                                ready.replace(args.output / "ready")
            finally:
                engine.stdin.close()
                engine.wait(timeout=5)
                report = dict(counts, peak_a=peaks[0], peak_b=peaks[1], mode=args.mode, snr_db=args.snr,
                              reference_peaks=args.peaks, seed=args.seed, sample_rate=RATE, block_size=BLOCK,
                              rx_gain_db=args.rx_gain_db, rx_peak_a=rx_peaks[0], rx_peak_b=rx_peaks[1],
                              calibrated_tx_peaks=args.peaks, normalized_peak=REFERENCE_PEAK if args.normalize else None,
                              tx_gain=tx_gain, impairment_started_monotonic=impairment_started,
                              impairment_aligned=args.align, engine_exit_code=engine.returncode,
                              impulse_period_seconds=0.4 if impulse else None,
                              impulse_width_seconds=0.04 if impulse else None,
                              fault_times_monotonic=fault_times,
                              finished_monotonic=time.monotonic(),
                              routing="process loopback" if args.pids else "stereo cable",
                              utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()))
                (args.output / "audio.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
