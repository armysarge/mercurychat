# M5 built-in lab checks

`run-builtin-lab.sh` runs the real MercuryChat service, headless
(`crates/mc-core/examples/builtin_lab.rs`), with its **own** Mercury and
rigctld from the Linux bundle (`lab/docker/build-bundle.sh`). The rig is
Hamlib's dummy (model 1) and the sound is two crossed PulseAudio null sinks:
no transceiver, no serial port, no RF.

| # | Check | What it proves (M5 design) |
|---|-------|----------------------------|
| 1 | Both stations reach Listening with their own rigctld running, take a sound card through `set_audio_config`, and every listening socket of a `mercury` or `rigctld` process is on 127.0.0.1 (read from `/proc`) | §5.3 private ini (`bind_address = 127.0.0.1`); §5.4 order: rigctld, then Mercury; §5.5 first start with no audio, then the card chosen |
| 2 | A calls B; a 4 KiB file arrives byte-exact | the built-in modem carries a session and a file like any Mercury |
| 3 | A service killed with `kill -9` leaves no `mercury` or `rigctld` process 3 s later | §5.4 children tied to the app (Linux parent-death signal) |
| 4 | A profile whose control port another process holds is refused with "Port 8700 is already in use on this computer.", and nothing connects to the holder | §5.1 never adopt a listener |

Expected: `4 of 4` in `summary.txt`.

## Running it

```bash
# The bundle (once): see lab/README.md §3d. Then build the lab program:
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/src:ro" -v mc-app-build-amd64:/build \
  -v "$HOME/.cargo/registry:/hostreg:ro" -v "$PWD/lab/docker/rig:/s:ro" \
  -v "E:/mc-lab-out/m5/builtin-lab-bin:/out" mercurychat-app-build:amd64 bash /s/build-builtin-lab.sh
# and run the checks:
MSYS_NO_PATHCONV=1 docker run --rm -v "E:/mc-lab-out/m5/bundle/linux-amd64:/bundle:ro" \
  -v "E:/mc-lab-out/m5/builtin-lab-bin:/lab:ro" -v "$PWD/lab/docker/rig:/scripts:ro" \
  -v "E:/mc-lab-out/m5/builtin-lab-N:/out" mercurychat-lab:rig bash /scripts/run-builtin-lab.sh
```

## The contracts beside it

`crates/mercury-contract/tests/contracts/builtin.rs` (real target only; the
contract lab sets `MC_MERCURY_BIN` and `MC_LAB_IP`): a Mercury started from
MercuryChat's generated ini is not reachable on the container's own address,
refuses a WebSocket upgrade with a foreign `Origin` (403) and accepts one with
none, reaches READY with `null` audio, lists devices with `-z` under `null` and
`pulse`, reports a missing sink as `audio_ok: false` while the TNC still
answers, and ends when stopped.

## Results

- 23 Nov 2026, bundle Mercury v1.9.25 (33efac4 + 0001/0002), Hamlib 4.7.2:
  lab `4 of 4` (E:/mc-lab-out/m5/builtin-lab-2); contracts 40 PASS, 1 SKIP,
  with the six built-in contracts passing; Mercury quit in 49 ms on SIGTERM
  (E:/mc-lab-out/m5/contracts-t6/run).
