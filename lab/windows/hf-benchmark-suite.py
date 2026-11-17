"""Run matched off-air A/B trials of Windows Mercury V2 and VARA HF builds."""

import argparse
from base64 import b64encode
from collections import defaultdict
import csv
from hashlib import sha256
from itertools import combinations, product
import json
import math
import os
from pathlib import Path
import re
import shutil
from statistics import median
import subprocess
import sys


HERE = Path(__file__).resolve().parent
LAB = HERE.parent
SOURCES = LAB / "run" / "hf-benchmark-sources"
PROFILES = ("CLEAN", "WGN", "IMPULSE", "MPG", "MPM", "MPP", "MPD")
PROFILE_NAMES = {
    "CLEAN": "Clean channel", "WGN": "White noise", "IMPULSE": "Impulsive interference",
    "MPG": "HF fading (MPG)", "MPM": "HF fading (MPM)",
    "MPP": "Poor multipath (MPP)", "MPD": "HF fading (MPD)",
}
DISPLAY_LABELS = {"VARA-HF": "VARA HF", "stock": "Mercury (stock)", "patched": "Mercury (patched)"}
BENCHMARK_COLORS = ("#BA4A19", "#94641C", "#087E73", "#775597", "#4C677C")
FAULT_KEYS = ("sample_errors", "input_overflow", "output_underrun")
OUTCOMES = ("matching_bytes", "missing_bytes", "corrupted_bytes", "duplicated_bytes",
            "reordered_bytes", "extra_bytes", "unframed_wire_bytes")


def read_json(path):
    if not path.is_file():
        return {}
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def write_csv(path, rows):
    columns = list(dict.fromkeys(key for row in rows for key in row if not isinstance(row[key], (dict, list))))
    with path.open("w", encoding="utf-8", newline="") as output:
        if columns:
            writer = csv.DictWriter(output, columns, extrasaction="ignore")
            writer.writeheader()
            writer.writerows(rows)


def file_hash(path):
    with path.open("rb") as source:
        digest = sha256()
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
        return digest.hexdigest()


def run_backend(params, variant, output, profile, snr, seed, reverse, peaks, calibration):
    settings = {
        "Modem": variant["modem"], "Output": str(output), "Bytes": 256 if calibration else params.bytes,
        "ConnectTimeout": params.connect_timeout, "Timeout": params.transfer_timeout,
        "Channel": "WGN" if profile == "CLEAN" else profile, "Seed": seed,
        "PayloadSeed": params.payload_seed, "RxGainDb": 0 if calibration else params.rx_gain_db,
        "IntegrityGrace": params.integrity_grace, "Engine": str(params.engine),
        "CaptureExe": str(params.capture), "AudioPython": str(params.audio_python), "Python": sys.executable,
        "ProcessLoopback": True, "Aligned": True, "TraceMercury": True, "Reverse": reverse,
        "DiscardVaraRuntime": True,
    }
    if variant["modem"] == "Mercury":
        settings["MercuryExe"] = variant["executable"]
        if params.mercury_audio:
            settings.update(MercuryTxDevice=params.mercury_audio[0], MercuryRxDevices=params.mercury_audio[1:])
    else:
        settings.update(VaraDir=str(Path(variant["executable"]).parent), Registered=not params.vara_unregistered)
    if snr is not None:
        settings.update(Noise=[snr, *peaks], Normalize=True)
    elif peaks is not None:
        # The clean suite profile uses the same transmit calibration as the noisy profiles.
        settings.update(Normalize=True, CalibratedPeaks=peaks)
    write_json(output.with_suffix(".settings.json"), settings)
    runner = str(HERE / "run-hf-benchmark.ps1").replace("'", "''")
    config = str(output.with_suffix(".settings.json")).replace("'", "''")
    script = (f"$settings = Get-Content -Raw -LiteralPath '{config}' | ConvertFrom-Json -AsHashtable; "
              f"& '{runner}' @settings; exit $LASTEXITCODE")
    encoded = b64encode(script.encode("utf-16-le")).decode("ascii")
    with output.with_suffix(".runner.log").open("wb") as log:
        code = subprocess.run([params.powershell, "-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
                              stdout=log, stderr=subprocess.STDOUT).returncode
    return code


def radio_stats(output, result, audio, reverse):
    path = output / "audio" / "ionos.log"
    engine_log = path.read_text(encoding="utf-8", errors="replace") if path.is_file() else ""
    footer = re.search(r"^BLOCKS (\d+) \([^)]+\), clipped (\d+)\s*$", engine_log, re.M)
    started = audio.get("impairment_started_monotonic")
    finished = result.get("finished_monotonic")
    connect = result.get("started_monotonic")
    stats = {"ionos_blocks": int(footer[1]) if footer else None,
             "ionos_clipped_samples": int(footer[2]) if footer else None,
             "connect_offset_seconds": connect - started if connect is not None and started is not None else None}
    if started is not None and connect is not None and finished is not None:
        bounds = (connect - started, finished - started)
        sender = "B" if reverse else "A"
        bursts = defaultdict(list)
        for station, time, duration in re.findall(r"^BURST ([AB]) at ([\d.]+) s, (\d+) ms,", engine_log, re.M):
            start = float(time)
            end = start + int(duration) / 1000
            overlap = min(end, bounds[1]) - max(start, bounds[0])
            if overlap > 0:
                bursts[station].append(overlap)
        for prefix, station in (("sender", sender), ("receiver", "A" if reverse else "B")):
            stats[f"{prefix}_rf_bursts"] = len(bursts[station])
            stats[f"{prefix}_airtime_seconds"] = sum(bursts[station])
    return stats


def retransmission_stats(output, modem, reverse):
    counts = {"retransmissions": None, "retransmission_unit": None,
              "connection_retries": None, "carousel_repoll_requests": None}
    sender = output / f"station-{int(reverse)}"
    receiver = output / f"station-{int(not reverse)}"
    if modem == "Mercury":
        path = sender / "mercury.log"
        tx_log = path.read_text(encoding="utf-8", errors="replace") if path.is_file() else ""
        path = receiver / "mercury.log"
        rx_log = path.read_text(encoding="utf-8", errors="replace") if path.is_file() else ""
        connect_transmissions = tx_log.count("[arq-fsm] state=CALLING ev=TX_STARTED")
        if connect_transmissions:
            counts["connection_retries"] = connect_transmissions - 1
        if "[carousel]" in rx_log:
            counts["carousel_repoll_requests"] = len(re.findall(r"\[carousel\] rx round lv=\d+ unheard: asking again", rx_log))
        # A build can declare that it emits exact payload retry counts during the trial.
        declaration = re.search(r"\bHF_BENCH_RETRANSMISSIONS unit=(frames|pieces)\b", tx_log)
        if declaration:
            events = re.findall(r"\bHF_BENCH_RETRANSMIT count=(\d+)\b", tx_log)
            counts.update(retransmissions=sum(map(int, events)), retransmission_unit=declaration[1])
    else:
        path = sender / "VARAHF.log"
        log = path.read_text(encoding="latin-1") if path.is_file() else ""
        attempts = re.findall(r"\bConnecting to \S+\.\.\. (\d+)/\d+", log)
        if attempts:
            counts["connection_retries"] = max(map(int, attempts)) - 1
    return counts


def invalid_reason(code, result, audio, radio, calibration):
    if code not in (0, 1) or "started_monotonic" not in result:
        return "Modem, audio bridge or control setup failed"
    if (code == 0) != (result.get("verified") is True):
        return "Runner exit status and ARQ result disagree"
    if not audio or audio.get("engine_exit_code") != 0 or radio["ionos_clipped_samples"] is None:
        return "Incomplete simulator diagnostics"
    if audio["finished_monotonic"] < result["finished_monotonic"] or radio["ionos_blocks"] != audio["impairment_blocks"]:
        return "Simulator stopped before measurement finished"
    counters = [audio[key] for key in FAULT_KEYS]
    faults = audio.get("fault_times_monotonic", [])
    if sum(counters) != len(faults):
        return "Incomplete audio fault timestamps"
    if any(result["started_monotonic"] <= event[0] <= result["finished_monotonic"] for event in faults):
        return "Audio bridge fault during measurement"
    if radio["connect_offset_seconds"] is None or not 0 <= radio["connect_offset_seconds"] <= 0.1:
        return "CONNECT outside the impairment start window"
    if radio["ionos_clipped_samples"] or audio["impulse_clipped_samples"] or audio["normalization_clipped_samples"]:
        return "Audio clipped"
    if calibration and not result.get("verified"):
        return "Clean calibration failed to verify the payload"
    return None


def attempt(params, variant, output, profile, snr, seed, reverse, peaks, calibration, number):
    label = variant["label"]
    direction = "B-to-A" if reverse else "A-to-B"
    print(f"{label:16} {profile:8} SNR {str(snr):>5}  seed {seed:<10} {direction}  attempt {number}", flush=True)
    code = run_backend(params, variant, output, profile, snr, seed, reverse, peaks, calibration)
    result = read_json(output / "arq" / "result.json")
    audio = read_json(output / "audio" / "audio.json")
    radio = radio_stats(output, result, audio, reverse)
    reason = invalid_reason(code, result, audio, radio, calibration)
    actual_hashes = []
    for index in range(2):
        binary = output / f"station-{index}/binary.txt"
        if binary.is_file():
            actual_hashes.append(binary.read_text().strip().removeprefix("sha256=").lower())
    if reason is None and actual_hashes != [variant["sha256"]] * 2:
        reason = "Modem binary changed after the suite was planned"
    matching = result.get("matching_bytes")
    transfer = result.get("transfer_seconds")
    row = dict(variant=label, modem=variant["modem"], binary_sha256=variant["sha256"], profile=profile,
               snr_db=snr, seed=seed, direction=direction, attempt=number, calibration=calibration,
               valid=reason is None, invalid_reason=reason, runner_exit_code=code,
               artifacts=output.relative_to(params.output).as_posix(), connected=result.get("connected"),
               verified=result.get("verified"), payload_sha256=result.get("sha256_sent"),
               wire_sent_sha256=result.get("sha256_wire_sent"), wire_received_sha256=result.get("sha256_received"),
               sent_bytes=result.get("sent_bytes"), wire_bytes_sent=result.get("wire_bytes_sent"),
               wire_bytes_received=result.get("wire_bytes_received"), goodput_bps=result.get("goodput_bps"),
               matching_bps=matching * 8 / transfer if matching is not None and transfer else None,
               setup_seconds=result.get("setup_seconds"), first_byte_seconds=result.get("first_byte_seconds"),
               transfer_seconds=result.get("transfer_seconds"),
               completion_seconds=result.get("connect_to_complete_seconds") if result.get("verified") else None,
               error=result.get("error") or result.get("transfer_error"),
               **{key: result.get(key) for key in OUTCOMES}, **radio,
               **retransmission_stats(output, variant["modem"], reverse))
    print(f"  {reason or ('verified' if row['verified'] else 'delivery failed')}", flush=True)
    return row, audio


def summarize(attempts, labels, plan):
    by_condition = defaultdict(dict)
    for row in attempts:
        if not row["calibration"]:
            condition = (row["profile"], row["snr_db"])
            key = (row["variant"], row["seed"], row["direction"])
            cases = by_condition[condition]
            if key not in cases or row["valid"]:
                cases[key] = row
    summary, pairs = [], []
    planned = defaultdict(set)
    for condition, seed, reverse in plan:
        planned[tuple(condition)].add((seed, "B-to-A" if reverse else "A-to-B"))
    for (profile, snr), slots in planned.items():
        cases = by_condition[profile, snr]
        valid = {(label, seed, direction): row for (label, seed, direction), row in cases.items() if row["valid"]}
        common = sorted(slot for slot in slots if all((label, *slot) in valid for label in labels))
        for label in labels:
            rows = [valid[(label, *slot)] for slot in common]
            delivered = [row for row in rows if row["verified"]]
            setup_rows = [row for row in rows if row["connected"]]
            first_bytes = [row["first_byte_seconds"] for row in rows if row["first_byte_seconds"] is not None]
            matching = [row["matching_bytes"] for row in rows if row["matching_bytes"] is not None]
            summary.append(dict(
                profile=profile, snr_db=snr, variant=label, planned_slots=len(slots),
                valid_slots=sum(variant == label for variant, _, _ in valid),
                matched_trials=len(rows), verified_transfers=len(delivered),
                connected_trials=len(setup_rows), median_matching_bytes=median(matching) if matching else None,
                delivery_success_rate=len(delivered) / len(rows) if rows else None,
                connection_success_rate=len(setup_rows) / len(rows) if rows else None,
                median_setup_seconds=median(row["setup_seconds"] for row in setup_rows) if setup_rows else None,
                median_first_byte_seconds=median(first_bytes) if first_bytes else None,
                median_completion_seconds=median(row["completion_seconds"] for row in delivered) if delivered else None,
                median_goodput_bps=median(row["goodput_bps"] for row in delivered) if delivered else None,
            ))
        for a, b in combinations(labels, 2):
            for seed, direction in sorted(slots):
                if (a, seed, direction) not in valid or (b, seed, direction) not in valid:
                    continue
                left, right = valid[a, seed, direction], valid[b, seed, direction]
                pairs.append(dict(profile=profile, snr_db=snr, seed=seed, direction=direction, a=a, b=b,
                                  a_verified=left["verified"], b_verified=right["verified"],
                                  **{f"{metric}_b_minus_a": right[metric] - left[metric]
                                     if left[metric] is not None and right[metric] is not None else None
                                     for metric in ("goodput_bps", "setup_seconds", "first_byte_seconds",
                                                    "completion_seconds", "matching_bytes", "sender_airtime_seconds")}))
    return summary, pairs


def snr_label(level):
    if level is None:
        return "Clean"
    value = f"{level:+g}".replace("-", "\N{MINUS SIGN}") if level else "0"
    return f"{value} dB"


def plot_delivery(plt, output, profile, rows, labels, colors, metadata):
    levels = sorted({row["snr_db"] for row in rows}, key=lambda level: 0 if level is None else level)
    lookup = {(row["snr_db"], row["variant"]): row for row in rows}
    payload = metadata["payload_bytes"]
    multiple_levels = len(levels) > 1
    group_span = len(labels) + (1.05 if multiple_levels else 0)
    chart_height = max(1.4, len(levels) * group_span * 0.58)
    footer = 1.10
    height = 1.40 + chart_height + footer
    figure = plt.figure(figsize=(10.6, height), facecolor="white")
    axis = figure.add_axes((0.25, footer / height, 0.49, chart_height / height))
    text_transform = axis.get_yaxis_transform()
    figure.text(0.045, 1 - 0.47 / height, "Payload delivered", fontsize=24, weight="bold", color="#172535")
    subtitle = PROFILE_NAMES[profile]
    if not multiple_levels and levels[0] is not None:
        subtitle += f" at {snr_label(levels[0])}"
    subtitle += f"  ·  {payload:,}-byte payload  ·  {metadata['transfer_timeout_seconds']:g} s to deliver after connection"
    figure.text(0.045, 1 - 0.81 / height, subtitle, fontsize=11, color="#47586A")

    for group, level in enumerate(levels):
        top = group * group_span
        if multiple_levels:
            count = lookup[level, labels[0]]["matched_trials"]
            axis.text(-0.065, top, f"{snr_label(level)}  ·  n={count}", transform=text_transform,
                      ha="right", va="center", fontsize=10, weight="bold", color="#47586A")
            top += 0.80
        for index, label in enumerate(labels):
            y = top + index
            row = lookup[level, label]
            axis.barh(y, payload, height=0.57, color="#EAF0F5", zorder=2)
            axis.text(-0.065, y, DISPLAY_LABELS.get(label, label), transform=text_transform,
                      ha="right", va="center", fontsize=11, weight="bold", color="#172535")
            matched = row["matched_trials"]
            matching = row["median_matching_bytes"]
            if matching is not None:
                axis.barh(y, matching, height=0.57, color=colors[label], zorder=3)
                amount = f"{matching:,.1f}".removesuffix(".0")
                value = f"{amount} / {payload:,} bytes"
            else:
                value = "\N{EM DASH}"
            if matched == 1:
                if not row["connected_trials"]:
                    detail = "No connection"
                elif row["verified_transfers"]:
                    detail = "Verified  ·  Connected"
                elif matching is not None and 0 < matching < payload:
                    detail = "Partial delivery  ·  Connected"
                else:
                    detail = "Unverified  ·  Connected"
            elif matched:
                detail = f"{row['verified_transfers']}/{matched} verified  ·  {row['connected_trials']}/{matched} connected"
            else:
                detail = "No matched trials"
            axis.text(1.05, y - 0.15, value, transform=text_transform, va="center",
                      fontsize=12, weight="bold", color="#172535")
            axis.text(1.05, y + 0.20, detail, transform=text_transform, va="center", fontsize=9, color="#47586A")

    axis.set(xlim=(0, payload), ylim=(len(levels) * group_span - 0.40, -0.60), yticks=[])
    axis.set_xticks(sorted({round(payload * fraction / 4) for fraction in range(5)}))
    axis.tick_params(axis="x", colors="#47586A", labelsize=9, length=0, pad=6)
    axis.set_xlabel("Median matching payload bytes" if any(row["matched_trials"] > 1 for row in rows)
                    else "Matching payload bytes", fontsize=10, color="#47586A", labelpad=8)
    axis.grid(axis="x", color="#E2E8EF", linewidth=0.8)
    for spine in axis.spines.values():
        spine.set_visible(False)

    plan = [(seed, reverse) for (name, _), seed, reverse in metadata["plans"] if name == profile]
    if len(plan) == 1 and rows[0]["matched_trials"] == 1:
        seed, reverse = plan[0]
        direction = "B \N{RIGHTWARDS ARROW} A" if reverse else "A \N{RIGHTWARDS ARROW} B"
        note = f"One matched trial per modem  ·  Seed {seed}  ·  {direction}"
    else:
        note = "Matched trials share the same channel profile, SNR, seed and transfer direction."
    figure.text(0.045, 0.25 / height, note, fontsize=9, color="#47586A")
    figure.savefig(output / f"{profile.lower()}.png", dpi=170)
    plt.close(figure)


def plot_summary(output, summary, labels, metadata):
    import matplotlib
    matplotlib.use("Agg")
    from matplotlib import pyplot as plt

    metrics = (("median_setup_seconds", "Connection setup (s)"), ("median_first_byte_seconds", "First byte (s)"),
               ("median_completion_seconds", "End-to-end delivery (s)"), ("median_goodput_bps", "Verified goodput (bits/s)"))
    mercury_labels = [label for label in labels if label != "VARA-HF"]
    colors = {label: BENCHMARK_COLORS[index % len(BENCHMARK_COLORS)] for index, label in enumerate(mercury_labels)}
    colors["VARA-HF"] = "#205FA6"
    for profile in dict.fromkeys(row["profile"] for row in summary):
        figure, axes = plt.subplots(2, 2, figsize=(9, 5.7), layout="constrained")
        all_rows = [row for row in summary if row["profile"] == profile]
        plot_delivery(plt, output, profile, all_rows, labels, colors, metadata)
        levels = sorted({row["snr_db"] for row in all_rows}, key=lambda snr: snr if snr is not None else 0)
        width = 0.8 / len(labels)
        for axis, (metric, title) in zip(axes.flat, metrics):
            for index, label in enumerate(labels):
                lookup = {row["snr_db"]: row[metric] for row in all_rows if row["variant"] == label}
                values = [math.nan if lookup.get(level) is None else lookup[level] for level in levels]
                positions = [x + (index - (len(labels) - 1) / 2) * width for x in range(len(levels))]
                bars = axis.bar(positions, values, width * 0.9, label=DISPLAY_LABELS.get(label, label), color=colors[label])
                zeroes = [x for x, value in zip(positions, values) if value == 0]
                axis.scatter(zeroes, [0] * len(zeroes), marker="_", color=bars[0].get_facecolor(), clip_on=False)
            axis.set_xticks(range(len(levels)), [snr_label(level) for level in levels])
            axis.set_title(title)
            axis.set_ylim(bottom=0)
            axis.grid(axis="y", alpha=0.3)
            if not any(row[metric] is not None for row in all_rows):
                axis.text(0.5, 0.5, "No comparable data", transform=axis.transAxes, ha="center")
        axes[0, 0].legend(fontsize=8, loc="best")
        figure.suptitle(f"{PROFILE_NAMES[profile]} · Median timing and goodput")
        figure.savefig(output / f"{profile.lower()}-timing.png", dpi=150)
        plt.close(figure)


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mercury", metavar="LABEL=EXE", action="append", default=[])
    parser.add_argument("--vara", type=Path, metavar="INSTALL_DIR")
    parser.add_argument("--vara-unregistered", action="store_true", help="Include VARA without the licensed callsign")
    result_dir = parser.add_mutually_exclusive_group(required=True)
    result_dir.add_argument("--output", type=Path, help="New result directory")
    result_dir.add_argument("--replot", type=Path, help="Regenerate PNGs from an existing suite.json")
    parser.add_argument("--profiles", nargs="+", choices=PROFILES, default=["WGN", "IMPULSE", "MPP"])
    parser.add_argument("--snr", nargs="+", type=float, default=[3, 0, -3])
    parser.add_argument("--seeds", nargs="+", type=int, default=[4243, 4244, 4245])
    parser.add_argument("--directions", choices=("forward", "reverse", "both"), default="forward")
    parser.add_argument("--bytes", type=int, default=1024)
    parser.add_argument("--payload-seed", type=int, default=4242)
    parser.add_argument("--connect-timeout", type=int, default=60)
    parser.add_argument("--transfer-timeout", type=int, default=180)
    parser.add_argument("--integrity-grace", type=float, default=5.0)
    parser.add_argument("--rx-gain-db", type=float, default=-18)
    parser.add_argument("--invalid-retries", type=int, default=1)
    parser.add_argument("--engine", type=Path, default=SOURCES / "ionos_pipe_48k_gain.exe")
    parser.add_argument("--capture", type=Path, default=SOURCES / "hf-process-audio.exe")
    installed_audio_python = Path(r"C:\VARA\OpenHF\.venv\Scripts\python.exe")
    parser.add_argument("--audio-python", type=Path, default=installed_audio_python if installed_audio_python.is_file()
                        else Path(sys.executable))
    parser.add_argument("--mercury-audio", nargs=3, metavar=("TX", "RX_A", "RX_B"),
                        help="Override the lab's Mercury WASAPI endpoint IDs. Use CABLE Input, CABLE-B Output and CABLE-A Output.")
    parser.add_argument("--powershell", default="pwsh")
    args = parser.parse_args()
    if args.replot:
        return args, []
    if sys.platform != "win32":
        parser.error("The virtual audio runner requires Windows")
    if not all(math.isfinite(value) for value in (args.integrity_grace, args.rx_gain_db, *args.snr)):
        parser.error("SNR, gain and grace must be finite")
    if not (-30 <= args.rx_gain_db <= 0 and args.integrity_grace > 0 and args.invalid_retries >= 0):
        parser.error("Use RX gain between -30 and 0, positive grace and nonnegative retries")
    if min(args.bytes, args.connect_timeout, args.transfer_timeout) <= 0:
        parser.error("Payload size and timeouts must be positive")
    if any(not 0 <= seed <= 2147483647 for seed in args.seeds) or not -2147483648 <= args.payload_seed <= 2147483647:
        parser.error("Seeds must fit a signed 32-bit integer; channel seeds must be nonnegative")
    args.output = args.output.resolve()
    if args.output.exists():
        parser.error("Use a new output directory")
    if not shutil.which(args.powershell):
        parser.error("PowerShell 7 (pwsh) is required")
    for key in ("engine", "capture", "audio_python"):
        path = getattr(args, key).resolve()
        if not path.is_file():
            parser.error(f"Missing --{key.replace('_', '-')} at {path}. See lab/README.md.")
        setattr(args, key, path)

    if not args.mercury:
        args.mercury = [f"Mercury={LAB / 'bin-patched/windows-x64/mercury.exe'}"]
        args.vara = args.vara or Path(r"C:\VARA")
    variants = []
    for item in args.mercury:
        label, equal, executable = item.partition("=")
        if not equal or not re.fullmatch(r"[a-zA-Z0-9_-]+", label) or not executable:
            parser.error("--mercury expects a label containing letters, numbers, '_' or '-' followed by '=EXE'")
        variants.append(dict(label=label, modem="Mercury", executable=str(Path(executable).resolve())))
    if args.vara:
        args.vara = args.vara.resolve()
        for name in ("VARA.exe", "VARA.ini", "Data.dat", "VARAHF500.dat", "VARAHF2300.dat", "VARAHF2750.dat"):
            if not (args.vara / name).is_file():
                parser.error(f"Missing {name} in --vara installation directory")
        variants.append(dict(label="VARA-HF", modem="VARA", executable=str(args.vara / "VARA.exe")))
    labels = [row["label"].casefold() for row in variants]
    if len(labels) < 2 or len(labels) != len(set(labels)):
        parser.error("Choose at least two modem variants with distinct labels")
    for variant in variants:
        binary = Path(variant["executable"])
        if not binary.is_file():
            parser.error(f"Missing simulator test binary {binary}")
        variant["sha256"] = file_hash(binary)
    return args, variants


def main():
    params, variants = arguments()
    if params.replot:
        output = params.replot.resolve()
        report = read_json(output / "suite.json")
        if not report:
            raise RuntimeError(f"No suite.json found in {output}")
        os.environ.setdefault("MPLCONFIGDIR", str(output / ".matplotlib"))
        labels = [variant["label"] for variant in report["variants"]]
        summary, _ = summarize(report["attempts"], labels, report["plans"])
        plot_summary(output, summary, labels, report)
        return 0
    try:
        audio_versions = subprocess.check_output(
            [str(params.audio_python), "-c",
             "import json, numpy, sounddevice; print(json.dumps(dict(numpy=numpy.__version__, sounddevice=sounddevice.__version__)))"],
            text=True, stderr=subprocess.PIPE)
    except subprocess.CalledProcessError as error:
        raise RuntimeError("Install numpy and sounddevice in --audio-python") from error
    params.output.mkdir(parents=True)
    os.environ.setdefault("MPLCONFIGDIR", str(params.output / ".matplotlib"))
    import matplotlib  # Check graph dependency before running hardware trials.
    conditions = [(profile, snr) for profile in dict.fromkeys(params.profiles)
                  for snr in ([None] if profile == "CLEAN" else dict.fromkeys(params.snr))]
    directions = {"forward": [False], "reverse": [True], "both": [False, True]}[params.directions]
    cases = list(product(conditions, dict.fromkeys(params.seeds), directions))
    metadata = dict(schema_version=1, variants=variants, plans=cases, payload_bytes=params.bytes,
                    payload_seed=params.payload_seed, connect_timeout_seconds=params.connect_timeout,
                    transfer_timeout_seconds=params.transfer_timeout, integrity_grace_seconds=params.integrity_grace,
                    rx_gain_db=params.rx_gain_db, engine_sha256=file_hash(params.engine),
                    capture_sha256=file_hash(params.capture), python=sys.version.split()[0],
                    matplotlib=matplotlib.__version__, audio_dependencies=json.loads(audio_versions),
                    scripts_sha256={path.name: file_hash(path) for path in
                                    (Path(__file__), HERE / "hf-arq-benchmark.py", HERE / "hf-ionos-bridge.py",
                                     HERE / "run-hf-benchmark.ps1")})
    attempts = []
    calibrated = {}
    write_json(params.output / "manifest.json", metadata)

    def run_case(variant, title, profile, snr, seed, reverse, peaks=None, calibration=False):
        for number in range(1, params.invalid_retries + 2):
            output = params.output / f"{title}-{variant['label']}-r{number}"
            row, audio = attempt(params, variant, output, profile, snr, seed, reverse, peaks, calibration, number)
            attempts.append(row)
            with (params.output / "attempts.jsonl").open("a", encoding="utf-8") as log:
                log.write(json.dumps(row, allow_nan=False) + "\n")
            if row["valid"]:
                return row, audio
        return row, audio

    try:
        for variant in variants:
            label = variant["label"]
            row, audio = run_case(variant, "calibration", "CLEAN", None, params.seeds[0], False, calibration=True)
            if not row["valid"]:
                print(f"Skipping {label} after failed calibration. See {row['artifacts']}.runner.log.", flush=True)
                continue
            peaks = [audio["peak_a"], audio["peak_b"]]
            if any(not 0 < peak <= 1 for peak in peaks):
                print(f"Skipping {label} because calibration did not measure both transmit peaks.", flush=True)
                continue
            calibrated[label] = peaks
        for index, ((profile, snr), seed, reverse) in enumerate(cases):
            shift = index % len(variants)
            order = variants[shift:] + variants[:shift]
            for variant in order:
                if variant["label"] in calibrated:
                    title = f"trial-{index + 1:03d}-{profile.lower()}"
                    run_case(variant, title, profile, snr, seed, reverse, calibrated[variant["label"]])
    finally:
        labels = [variant["label"] for variant in variants]
        summary, pairs = summarize(attempts, labels, cases)
        write_json(params.output / "suite.json", dict(metadata, calibrated_tx_peaks=calibrated,
                                                      attempts=attempts, summary=summary, paired=pairs))
        write_csv(params.output / "trials.csv", attempts)
        write_csv(params.output / "summary.csv", summary)
        write_csv(params.output / "paired.csv", pairs)
        plot_summary(params.output, summary, labels, metadata)
    missing = len(cases) * len(variants) - sum(row["valid"] and not row["calibration"] for row in attempts)
    print(f"Results: {params.output}. Unusable benchmark slots: {missing}.")
    return int(missing > 0)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as error:
        print(f"Benchmark error: {error}", file=sys.stderr)
        raise SystemExit(2)
