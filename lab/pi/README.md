# Raspberry Pi M0 proof kit

Status: **prepared, not yet run** — no Pi was available on 2026-11-14. The Pi gate in [docs/07 §8](../../docs/07-virtual-audio-and-raspberry-pi.md) stays open until this runs on real hardware.

## Target

Raspberry Pi 4 or 5, 64-bit Raspberry Pi OS (trixie) **desktop**, logged in to the graphical session. Record power supply, cooling and storage; the script records the rest.

## Assemble the kit (on the build PC)

```bash
mkdir -p pi-kit && cp lab/pi/pi-proof.sh lab/bin/linux-arm64/mercury lab/bin/app-linux/arm64/m0-client lab/bin/app-linux/arm64/MercuryChat_*_arm64.deb pi-kit/
```

## Run (on the Pi, in a desktop terminal)

```bash
cd pi-kit && chmod +x pi-proof.sh mercury m0-client && ./pi-proof.sh --minutes 10
```

It asks for `sudo` once to install the `.deb`. Nothing transmits RF and no physical PTT is configured: both Mercury instances use `-P none` and talk through two temporary PipeWire null sinks, which are removed at the end.

## What it measures

| File | Contents |
|---|---|
| `baseline.txt` | Model, RAM, OS, kernel, 64-bit userland, CPU features/part, session type, throttle/temperature at start and end, WebKitGTK version, audio server |
| `binaries.txt` | ELF type, sha256 and library resolution for Mercury and the client; package Depends |
| `startup.txt` | Three app launches: `MC_UI_READY_MS` (process start → first rendered data) and IPC round-trip p50/p95/max over 200 calls |
| `client/report.md` | The full M0 scenario (connect, text/binary, receipts, dedup, file integrity, disconnect, recovery) over the Pi's crossed audio routes **while the app shell is open** |
| `resources.log` | Every 2 s: CPU%/RSS of the app, WebKit processes and both Mercury processes, temperature, throttle flags, load |

Copy the `pi-results-*` folder back and add the figures to `docs/M0-progress.md`. Compare against the provisional Pi budgets in docs/07 §7 (shell ready ≤ 5 s, interaction p95 ≤ 150 ms).

## Build baseline of the arm64 artifacts

The arm64 Mercury is a generic ARMv8-A build with `-moutline-atomics` (no `-mcpu=cortex-a72/a76`), so the same binary is meant to run on Pi 4 and Pi 5. Run the kit on a Pi 4 before claiming Pi 4 support (PI-02).
