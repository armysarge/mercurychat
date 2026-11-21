# M0 lab

Everything needed to reproduce the M0 modem-integration proof. Results and findings are logged in [docs/M0-progress.md](../docs/M0-progress.md).

Requirements on the build PC: Rust (rustup, MSVC on Windows), Node.js 20+, Docker Desktop. Nothing here keys a radio: every Mercury instance runs with PTT `none`.

## Mercury V2 A/B benchmark on Windows

Run from the repository root in PowerShell. This example compares two Mercury builds and registered VARA HF in one command.

```powershell
python lab/windows/hf-benchmark-suite.py --output lab/run/hf-ab-01 --mercury stock=lab/bin/windows-x64/mercury.exe --mercury patched=lab/bin-patched/windows-x64/mercury.exe --vara C:/VARA
```

Use a new output directory every time. Repeat `--mercury LABEL=EXE` for each build and omit `--vara` for a Mercury-only comparison. The default sweep uses `WGN IMPULSE MPP` at +3, 0 and −3 dB over seeds 4243–4245. Supply `--profiles`, `--snr` and `--seeds` to change the matrix. Add `--directions both` to repeat in the reverse direction. For a short infrastructure smoke test append `--snr 10 --seeds 4243 --bytes 128`. The suite calibrates each binary once through the clean audio route before starting the matrix.

### One-time setup

- Use PowerShell 7 and Python 3.10 or newer. Install NumPy, SoundDevice and Matplotlib with `python -m pip install numpy sounddevice matplotlib`. The lab defaults to the OpenHF virtual environment for bridge audio when present. Set `--audio-python` to select a different interpreter with NumPy and SoundDevice.
- Install VB-Audio Cable, Cable A and Cable B. Enable Windows desktop-app microphone access for the virtual capture endpoints. Both transmitters play to Cable Input, which the bridge captures per process. The bridge renders into Cable B for station A and Cable A for station B. On another PC pass `--mercury-audio TX RX_A RX_B` with the three Mercury WASAPI endpoint IDs in that order.
- Supply Mercury Windows binaries and, for VARA trials, an existing VARA HF installation. The runner uses lab virtual COM2 and COM7 for isolated VARA instances, with RTS, DTR and radio-board keying disabled. `--vara-unregistered` uses synthetic calls and measures the unregistered modem configuration. TCP ports 19300–19301 and 19400–19401 must be free.
- Build the two bridge executables once from a Visual Studio x64 Developer command prompt with Rust installed. Existing executables can also be supplied with `--engine` and `--capture`.

```powershell
git clone https://github.com/jesseforkum/ion-hf.git lab/run/hf-benchmark-sources/ion-hf
git -C lab/run/hf-benchmark-sources/ion-hf switch --detach 1eb7dca01803e120eeb0924fd02d4ccc563557a4
pwsh -NoProfile -File lab/windows/build-hf-benchmark-tools.ps1
```

### Measurement and outputs

- **Matched channels.** The [ARSFI HFSimulator](https://github.com/ARSFI/HFSimulator) IONOS software port supplies 3 kHz-bandwidth white noise and the MPG, MPM, MPP and MPD HF fading profiles. `IMPULSE` uses seeded, tapered 40 ms noise bursts with a randomized position in every 400 ms interval and an average interference power corresponding to the selected SNR. Impulse-noise samples are added after the clean IONOS route. Each modem’s transmit peak is normalized to the same digital reference. SNR is referenced to a sine at that peak. The suite starts the seeded channel timeline at connection setup, measures the start offset and rotates run order. The channel series is reproducible. Different modem timing places bursts at different points in that series.
- **Payload and timing.** Both ends must report `CONNECTED` within the connection window. The sender uses one seeded application payload, numbered in blocks of up to 128 bytes. Exact wire equality determines success. Record IDs and byte alignment identify missing, corrupted, duplicate, reordered and extra data. With damaged record headers or ambiguous edits, the counts describe the inferred alignment and any unframed bytes. The harness observes the receiver for five extra seconds after first seeing the exact stream so it can detect trailing data. Verified goodput is application bits divided by transfer time. The matching-byte rate also covers partial transfers. The suite records separate times for setup, first byte, transfer and end-to-end delivery.
- **Retransmission telemetry.** The suite records connection retries, RF burst counts and airtime for both modems, plus Mercury’s carousel re-poll requests. Exact payload retries are available for instrumented Mercury builds. Emit `HF_BENCH_RETRANSMISSIONS unit=frames` or `HF_BENCH_RETRANSMISSIONS unit=pieces` on the sender at startup, then emit `HF_BENCH_RETRANSMIT count=N` for each actual retransmission. Stock Mercury and VARA HF results leave the payload retry count null. VARA HF’s host interface exposes buffer state and keying, and the installed build’s log reports connection attempts.
- **Results.** `suite.json` contains the settings, all attempts, summary rows and paired comparisons. `trials.csv`, `summary.csv` and `paired.csv` contain the corresponding tables. Each profile gets a payload delivery chart and a separate timing and goodput chart. Figures use the same valid seed/direction cohort for all displayed variants. Pairwise CSV rows keep any seed/direction that is valid for the two named variants. Per-trial directories hold ARQ transcripts, hashes, modem logs, expected and observed wire streams, and audio diagnostics. Regenerate the PNGs from saved results with `python lab/windows/hf-benchmark-suite.py --replot lab/run/hf-ab-01`.
- **Validity.** Audio faults inside the measurement window or simulator clipping invalidate an attempt. The suite saves those attempts and retries them with the same condition. The default receive gain is −18 dB for pulse headroom. Change `--rx-gain-db` to adjust signal and noise together. A valid modem timeout is a delivery failure and remains in the rate denominator. Exit 0 means the matrix collected a valid outcome for each slot. Exit 1 means at least one required slot remained invalid or lacked calibration. Raw modem logs can include station callsigns.

Run the offline payload and simulator regressions with `python -B lab/windows/test-hf-benchmark.py -v`.

## 1. Build Mercury (pinned source, three targets)

```bash
docker build -t mercurychat-lab:m0 lab/docker
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "F:/Programming/mercury:/src:ro" -v "$PWD/lab/bin:/out" -v "$PWD/lab/docker:/scripts:ro" mercurychat-lab:m0 bash -c 'git config --global --add safe.directory "*" && bash /scripts/build-mercury.sh'
```

Output: `lab/bin/{linux-amd64,linux-arm64,windows-x64}/` with `MANIFEST.txt` (commit, compiler, sha256). The script clones the pinned commit instead of copying the Windows checkout, whose CRLF endings break Mercury's build scripts.

### Mercury patches

`lab/mercury-patches/*.patch` are applied in order to the pinned commit by `build-mercury.sh` when mounted at `/patches`; each build's `MANIFEST.txt` lists them. Currently: upstream PR #565 (`bind_address`), PR #566 (WebSocket Origin/Host checks, `ui_allowed_origins`; `0002` is #566 merged onto #565) and the Hamlib reopen fix from the fork's `radio-hamlib-reopen` branch (`0003`, upstream PR #571). Patched builds go to `lab/bin-patched/`:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "F:/Programming/mercury:/src:ro" -v "$PWD/lab/bin-patched:/out" -v "$PWD/lab/docker:/scripts:ro" -v "$PWD/lab/mercury-patches:/patches:ro" mercurychat-lab:m0 bash -c 'git config --global --add safe.directory "*" && bash /scripts/build-mercury.sh'
```

Exposure check (Windows; null audio, PTT none): `bash lab/windows/exposure-check.sh <mercury.exe> <out-dir> [bind_address]`. WebSocket Origin/Host check (stock vs patched, four configurations): `python lab/windows/ws-origin-check.py lab/bin/windows-x64/mercury.exe lab/bin-patched/windows-x64/mercury.exe lab/run/ws-origin`. For the cable lab bound to loopback, add `BIND=127.0.0.1` and `M=lab/bin-patched/windows-x64/mercury.exe` to `run-win-lab.sh`.

### Testing Mercury changes for upstream

Changes meant for Mercury upstream live on branches of the owner's fork ([armysarge/mercury](https://github.com/armysarge/mercury)), cloned at `F:\Programming\mercury-fork`. `lab/docker/mercury-it/run-it.sh` builds the branch's merge base with upstream and the working tree, runs the named unit tests, and runs a Go integration test against both builds, so a new test is seen to fail before and pass after:

```bash
docker build -t mercurychat-lab:it lab/docker/mercury-it
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "F:/Programming/mercury-fork:/src:ro" -v "$PWD/lab/run/it-N:/out" -v "$PWD/lab/docker/mercury-it:/scripts:ro" mercurychat-lab:it bash /scripts/run-it.sh TestHamlibSessionSurvivesRigctldRestart test_radio_io_frequency test_radio_port
```

To run the rig lab on a fork build, build `linux-amd64` with `/src` mounted from the fork into `lab/bin-fork/<branch>` and mount that at `/mercury`. Add `-e EXPECT_REOPEN=1` for a build with the Hamlib reopen fix: the post-restart keying finding then becomes a check. PR drafts are in `lab/upstream/`.

## 2. L0 — fake Mercury pair (no audio, seconds)

```bash
cargo run -p m0-client -- selftest --work lab/run/selftest
```

## 3. L1 — two real Mercury instances over crossed virtual audio

```bash
MSYS_NO_PATHCONV=1 docker run -d --name mc-lab -p 127.0.0.1:8300-8301:8300-8301 -p 127.0.0.1:8400-8401:8400-8401 -v "$PWD/lab/bin:/mercury:ro" -v "$PWD/lab/run/l1:/out" -v "$PWD/lab/docker:/scripts:ro" mercurychat-lab:m0 bash /scripts/run-lab.sh
```

```bash
cargo run --release -p m0-client -- lab --a 127.0.0.1:8300 --b 127.0.0.1:8400 --work lab/run/l1/client --file-bytes 4096
```

`run-lab.sh` creates two PulseAudio null sinks (`a_to_b`, `b_to_a`), checks that each route carries signal only to its own monitor, writes one `mercury.ini` per station and starts both modems. Logs, the lab manifest and a 5 s CPU/RSS sample of both modems land in `lab/run/l1/`.

Do not write to a Mercury data port while no session is up: Mercury queues those bytes and sends them into the next session (see the progress log). The client purges such bytes with `ABORT` on attach.

## 3a. Rig control — Mercury keying through rigctld (dummy rig)

Hamlib's dummy rig (model 1) behind `rigctld -P RIG`, one per station; each Mercury keys PTT through it as a NET rigctl client (`-R 2 -A 127.0.0.1:<port>`). `rig-proof.py` plays MercuryChat: it reads and sets frequency and mode, watches PTT, checks Mercury's WebSocket telemetry and live `set_tx_gain`, runs a session, kills rigctld while keyed, and restarts Mercury to restore keying. No real radio or serial port is used.

```bash
docker build -t mercurychat-lab:rig lab/docker/rig
```

```bash
mkdir -p lab/run/rig-N && MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD/lab/bin:/mercury:ro" -v "$PWD/lab/run/rig-N:/out" -v "$PWD/lab/docker/rig:/scripts:ro" mercurychat-lab:rig bash /scripts/run-rig-lab.sh
```

Results: `lab/run/rig-N/report.md` (checks and findings), `report.json` (PTT timelines, control transcripts), Mercury and rigctld logs per station. About two minutes.

### M3b rig control: the app against a shared rigctld (dummy rig)

`crates/mc-core/examples/rig_lab.rs` runs the real service actor headless (no window) as station A, against two patched Mercury modems (0001–0003, in `lab/bin-patched`) that key the Hamlib dummy rig through rigctld. Station B is answered by the program's own headless peer. A third Mercury, set to a serial Hamlib model on a pseudo-terminal pair (`pty-pair.py`, a TS-2000 emulated by `rigctlcom`), is used for the two-owner check. No real serial device or radio is used.

Build the program for Linux (the `mercurychat-app-build:amd64` image from section 4), then run the lab:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/src:ro" -v mc-app-build-amd64:/build -v "$PWD/lab/bin/rig-lab:/out" -v "$PWD/lab/docker/rig:/scripts:ro" mercurychat-app-build:amd64 bash /scripts/build-rig-lab.sh
```

```bash
mkdir -p lab/run/m3b-rig-N && MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD/lab/bin-patched:/mercury:ro" -v "$PWD/lab/bin/rig-lab:/lab:ro" -v "$PWD/lab/run/m3b-rig-N:/out" -v "$PWD/lab/docker/rig:/scripts:ro" mercurychat-lab:rig bash /scripts/run-m3b-lab.sh
```

It checks:

1. Mercury keys through the shared rigctld and the PTT cross-check agrees, with no fault;
2. the app sets frequency and mode while idle and Mercury's telemetry follows (the time is reported; Mercury polls the rig every few seconds);
3. a frequency change is refused while connected, and no `F` reaches rigctld (read from its `-vvvv` log);
4. rigctld killed while keyed raises "lost while keyed", and after the restart there is at most one PTT off, none while Mercury reports PTT on;
5. rigctld killed while idle leaves the modem listening and the frequency falls back to telemetry;
6. the TX level for a band is applied and confirmed;
7. Tune keys the rig and stops by itself after about 30 s;
8. rigctld without `-P` turns the PTT check off, raises no fault and refuses Tune;
9. a Mercury with its own CAT connection is refused as a second owner, with the reason.

Results: `report-m3b.txt` (one PASS or FAIL line per check), `activity-m3b.txt`, and the rigctld and Mercury logs. About three minutes. `run-m3b-lab.sh` is now a wrapper for `run-rig-lab-checks.sh m3b`.

### M3b.1: MercuryChat's own rigctld, Mercury's PTT and audio

`run-rig-lab-checks.sh m3b1` runs seven checks:
- MercuryChat starts its own rigctld (dummy rig) and stops it on exit, or reuses one that's already there;
- it sets Mercury's PTT and audio over Mercury's UI link;
- it shows the frequency offset;
- check 7 repeats the run against unpatched upstream Mercury.

What each check proves, and the commands, are in [`docker/rig/M3b1-checks.md`](docker/rig/M3b1-checks.md). On Windows, `node lab/windows/m3b1-exit-check.mjs <mercurychat.exe> <fake-rigctld.exe> <empty dir>` checks that closing the real window stops the app's rigctld (with a fake rigctld; nothing must be listening on 4532).

## 4. Desktop shell

```bash
npm install && npm --prefix apps/desktop install
```

```bash
npx tauri build
```

Linux packages (one image per architecture, because WebKitGTK `-dev` packages are not multi-arch co-installable). Build the UI on the host first (`npm run build:ui`):

```bash
docker build -f lab/docker/Dockerfile.app --build-arg ARCH=arm64 -t mercurychat-app-build:arm64 lab/docker
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/src:ro" -v mc-app-build-arm64:/build -v "$PWD/lab/bin/app-linux:/out" -v "$PWD/lab/docker:/scripts:ro" mercurychat-app-build:arm64 bash /scripts/build-app-linux.sh arm64
```

Use `ARCH=amd64` / `build-app-linux.sh amd64` for the x86-64 package.

## 5. Raspberry Pi

See [pi/README.md](pi/README.md). Prepared, not yet run.

## Windows virtual-cable lab

Needs two independent virtual cables. This PC uses VB-Audio Cable A and Cable B: station A plays into CABLE-A Input and captures CABLE-B Output; station B the reverse. Both run natively with `-x wasapi` and `-P none`. Device ids come from `lab/bin/windows-x64/mercury.exe -x wasapi -z`; the script's defaults are this PC's ids and can be overridden with `CA_IN`, `CA_OUT`, `CB_IN`, `CB_OUT`.

```bash
bash lab/windows/run-win-lab.sh
```

It starts both modems, waits for their ports, samples their CPU and working set every 5 s, runs the same `m0-client lab` scenario, and stops the modems. Results land in `lab/run/win-l1/`. Use a fresh `OUT=lab/run/win-l1-N` for each run. Windows Firewall may ask about `mercury.exe`; declining is fine because the lab only uses loopback.

To run the desktop shell alongside both modems (one long-lived window plus four timed launches, with process-tree sampling), use `bash lab/windows/run-shell-with-modems.sh`. Set `OUT`, `DATA`, `APP`, `M` and `CLIENT` to put everything on an SSD: on this PC F: is a busy HDD, which slowed the shell and receipts when it was saturated (see the progress log).

Broadcast and discovery proof (CQ both ways, MCB/1 beacons, capacity and session gate, BUSY, MONITOR):

```bash
OUT=lab/run/win-bcast-N RUN='target/release/m0-client.exe broadcast --a 127.0.0.1:8300 --b 127.0.0.1:8400 --work "$OUT/client"' bash lab/windows/run-win-lab.sh
```

The exploratory probe that produced the design is `RUN='python lab/windows/broadcast-probe.py "$OUT"'`. For the slow-mode scenario add `CLIENT_ARGS='--bw 500'`.

To re-check route isolation, run Mercury's TX test (`-t`) into one cable's input with RX tests (`-r`) on both cable outputs; only the matching output should decode frames (see `lab/run/win-iso`).

### M3a discovery over two Mercury modems (no RF)

`lab/windows/m3-discovery.mjs` runs the M3a discovery checks against two real modems and two MercuryChat windows in lab mode. It starts both `mercury.exe` itself (the crossed VB-Audio cables as above, WASAPI, PTT `none`, `bind_address = 127.0.0.1`, telemetry WebSocket on with `-G -U 10000` / `-U 10001`), then two app instances (`MC_DATA_DIR` and a WebView2 profile each, DevTools on 9351/9352), saves an Existing profile in each and drives them by invoking the app's typed commands in the page. It reads the heard list, beacon status, telemetry view, activity ring, events and QSO drafts from the app; it does not use the Operate screens.

Every app-to-modem socket goes through a loopback relay the script runs (control 8310/8410, data 8311/8411, broadcast 8110/8210), which logs each control line with its time: those transcripts, the modem logs and the app's events are the evidence. Station B's telemetry goes through its own relay (10011), which the script closes mid-session. A plain WebSocket client on each modem's UI port records how the status JSON fields are written.

Build the app with its assets embedded, then run it with a fresh run directory (Mercury locks files in an old one):

```bash
cargo build -p mercurychat --features tauri/custom-protocol
```

```bash
M=lab/bin-patched/windows-x64/mercury.exe node lab/windows/m3-discovery.mjs target/debug/mercurychat.exe lab/run/m3-disc-N
```

It checks:

- every CQ is heard as one row with source CQ and the MONITOR SNR, whichever of CQFRAME and MONITOR comes first;
- no telemetry SNR is shown before Mercury has decoded anything;
- with Mercury A's broadcast port held by a raw client, the first Beacon now times out and resets the connection. A second attempt is refused and beacons hold. Releasing the raw client must produce zero queued frames and zero beacons heard on B. Re-arming opens a fresh connection;
- a beacon is heard with its locator;
- the session gate: beacon toggle, Beacon now and CQ are refused during a session, and an armed beacon that falls due inside it never reaches the air;
- exactly one beacon follows one interval (5 min, `BEACON_MIN`) after the session;
- telemetry is lost mid-session while chat keeps delivering both ways;
- frequency is unavailable without a rig (telemetry, QSO drafts, heard rows);
- session ends reach the other side promptly, with Receive only chosen while the call rings or kept throughout;
- Receive only switched on mid-session is shown as pending, sends no LISTEN while linked, lets chat continue, and sends one LISTEN OFF after the graceful end.

The stale-frame check covers a Windows Mercury receiver. A Linux receiver can read buffered data before it sees a TCP reset, so the Windows result does not qualify a Linux modem for this case.

It also records the CQFRAME vs MONITOR order, PENDING/CANCELPENDING around our CQ, a second client on a broadcast port, and the status JSON's types. Results: `report.md` and `report.json` in the run directory, modem logs in `a/` and `b/`. About 22 minutes. Set `MC_LAB_BROADCAST_ONLY=1` to run only the discovery and broadcast checks before the session tests. Do not touch the two windows while it runs (commands the windows send themselves are logged and fail the run).
