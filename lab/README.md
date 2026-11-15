# M0 lab

Everything needed to reproduce the M0 modem-integration proof. Results and findings are logged in [docs/M0-progress.md](../docs/M0-progress.md).

Requirements on the build PC: Rust (rustup, MSVC on Windows), Node.js 20+, Docker Desktop. Nothing here keys a radio: every Mercury instance runs with PTT `none`.

## 1. Build Mercury (pinned source, three targets)

```bash
docker build -t mercurychat-lab:m0 lab/docker
```

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "F:/Programming/mercury:/src:ro" -v "$PWD/lab/bin:/out" -v "$PWD/lab/docker:/scripts:ro" mercurychat-lab:m0 bash -c 'git config --global --add safe.directory "*" && bash /scripts/build-mercury.sh'
```

Output: `lab/bin/{linux-amd64,linux-arm64,windows-x64}/` with `MANIFEST.txt` (commit, compiler, sha256). The script clones the pinned commit instead of copying the Windows checkout, whose CRLF endings break Mercury's build scripts.

### Mercury patches

`lab/mercury-patches/*.patch` are applied in order to the pinned commit by `build-mercury.sh` when mounted at `/patches`; each build's `MANIFEST.txt` lists them. Currently: upstream PR #565 (`bind_address`) and PR #566 (WebSocket Origin/Host checks, `ui_allowed_origins`; `0002` is #566 merged onto #565). Patched builds go to `lab/bin-patched/`:

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
