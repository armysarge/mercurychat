# M3b.1 rig lab checks

What each check of `run-rig-lab-checks.sh m3b1` proves, with the real MercuryChat
service (`crates/mc-core/examples/rig_lab.rs`, headless) against real Mercury
and Hamlib in the `mercurychat-lab:rig` image. There's no radio, serial device
or RF anywhere:
- the rig is Hamlib's dummy (model 1, with `-r /dev/null`);
- audio is PulseAudio null sinks;
- station B is the program's own headless peer behind a second Mercury.

**Setup:**
- Mercury A starts with `-P none` (no PTT) and `-G`, and nothing listens on 4532.
- Mercury B keys its own rigctld on 4632.
- Station A's rig settings are Frequency control "Hamlib (MercuryChat runs rigctld)", model 1, PTT "CAT" and Hamlib folder `/usr/bin`.

| # | Check | Proves | Pass |
|---|---|---|---|
| 1 | managed | MercuryChat finds rigctld, starts it with the settings' flags on 127.0.0.1:4532, and reads the rig through it. On exit (`Shutdown`, as the window sends) it stops it. | daemon `running`, link up, one `rigctld -m 1 … -t 4532` process; the script then finds **no** rigctld on 4532 after `rig_lab` exits |
| 2 | reuse | A rigctld already on 4532 (someone else's) is used, never started over and never stopped. | daemon `reused`; that rigctld still runs after `rig_lab` exits |
| 3 | PTT push | Mercury's PTT is set to match the rig settings. Mercury keys the radio, MercuryChat only tells it how. | Mercury's own `radio_list` (read by a second UI client, `ws-snoop.py`) says `ptt_method hamlib`, `device_path 127.0.0.1:4532`; the activity log notes it; during a session a separate `rigctl -m 2 -r 127.0.0.1:4532 t` reads `1` |
| 4 | audio | Settings → Audio's lists are Mercury's. A change Mercury accepts is saved by Mercury, and one it rejects is reported and not saved. | inputs hold `a_to_b.monitor`, outputs `b_to_a`; setting them gives `audio_ok: true` and Mercury A's `mercury.ini` holds both; a bogus input gives `audio_ok: false` with the ini unchanged; the crossed routing is restored the same way |
| 5 | offset | The displayed frequency is the dial plus the offset. | dummy dial 14 100 000 (set by `rigctl`), offset +1500 → the rig view shows 14 101 500 |
| 6 | FLRig | Skipped here: Linux FLRig needs a GUI. Covered by `FakeFlrig` (`crates/mercury-adapter/tests/flrig.rs`) and by `flrig_backend_reads` in `crates/mc-core/tests/rig.rs`. | n/a |
| 7 | upstream | Checks 3 and 4 hold against upstream Mercury without MercuryChat's patches. | the same run against the unpatched build (see `lab/upstream/M3b1-divergence.md`) |

The check ran first without a shutdown path. Check 1's "after exit" failed: the
app's rigctld outlived it, because a dropped handle's kill never runs once the
process ends. `RigctldDaemon::shutdown` and `AppCommand::Shutdown`, sent from
Tauri's `RunEvent::Exit`, fixed it. On Windows, `lab/windows/m3b1-exit-check.mjs`
proves the same for the real window, with a fake rigctld.

## Run

Build Mercury (any checkout of the fork; `MERCURY_COMMIT` pins the commit) and
`rig_lab`, then run the checks. Outputs go on E:, never F:.

```bash
MSYS_NO_PATHCONV=1 docker run --rm -e MERCURY_COMMIT=f77adfb -v "E:/Programming/mercury-fork:/src:ro" -v "E:/mc-lab-out/m3b1/bin-patched:/out" -v "$PWD/lab/docker:/scripts:ro" -v "<dir with 0001 and 0002>:/patches:ro" mercurychat-lab:m0 bash -c 'git config --global --add safe.directory "*" && bash /scripts/build-mercury.sh linux-amd64'
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/src:ro" -v mc-app-build-amd64:/build -v "E:/mc-lab-out/m3b1/rig-lab:/out" -v "$PWD/lab/docker/rig:/scripts:ro" mercurychat-app-build:amd64 bash /scripts/build-rig-lab.sh
MSYS_NO_PATHCONV=1 docker run --rm -v "E:/mc-lab-out/m3b1/bin-patched:/mercury:ro" -v "E:/mc-lab-out/m3b1/rig-lab:/lab:ro" -v "E:/mc-lab-out/m3b1/run-patched:/out" -v "$PWD/lab/docker/rig:/scripts:ro" mercurychat-lab:rig bash /scripts/run-rig-lab-checks.sh m3b1
```

For check 7, build without `/patches` into `bin-upstream` and run again with
that build mounted at `/mercury`.

Patch 0003 (the Hamlib reopen) is upstream at f77adfb (#571, #574), so the
patched build there carries only 0001 and 0002.

Results: `report.txt` (the seven lines), `report-m3b1.txt`, `report-reuse.txt`,
`activity-*.txt`, `lab-manifest.txt`, and each Mercury's and rigctld's logs.
A run takes about two minutes.
