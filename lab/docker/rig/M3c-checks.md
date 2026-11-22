# M3c rig lab checks

What each check of `run-rig-lab-checks.sh m3c` proves. The lab runs two real MercuryChat
service instances (`crates/mc-core/examples/rig_lab.rs` mode `m3c-qsy`, headless) against two
real Mercury modems and two Hamlib rigctld processes in the `mercurychat-lab:rig` image.
There's no radio, serial device or RF anywhere:
- each rig is Hamlib's dummy (model 1);
- audio is PulseAudio null sinks;
- Mercury keys each dummy rig through its own rigctld (`-R 2 -A`).

**Setup:**
- Station A is `ZS6LAB`: TNC 8300, telemetry 10000, broadcast 8100, rigctld 4532, offset 0.
- Station B is `ZS6PEER`: TNC 8400, telemetry 10100, broadcast 8200, rigctld 4632, offset +1500.
- Both rigs start on a dial of 7 105 000 USB. The QSY windows are the real ones: 60 s to answer, a 120 s re-call, and 20 s grace for the answerer.
- Rig frequencies are read by a separate `rigctl` client, not by MercuryChat.

| # | Check | Proves | Pass |
|---|---|---|---|
| 5 | kind 2 and kind 3 | MCB/1 kinds 2 and 3 cross the real broadcast plane (DATAC3). | B's extended beacon (name, QTH, status "QRV 40m") shows those fields in A's heard list. A's suggestion to ZS6PEER reaches B as `QsySuggested` from ZS6LAB, at 7 110 000 USB. |
| 1 | QSY end to end | Propose, accept, the confirming echo, the caller's disconnect, the retune, the re-call. | qsy/1 is negotiated. A's dial goes to 7 120 000 and B's to 7 118 500 (the candidate minus each offset). A's re-call reaches CONNECTED with a new session ID. Both QSY cards clear. |
| 4 | info | SNR and station-info requests are answered across the link, and the email is not in the answer. | A gets `PeerInfo` with a finite SNR, and B's name and QTH, with no email, although B's profile has one. |
| 3 | gate | A gate that fails at the accept declines with `rig` and never disconnects. | A proposes; once B is offered, A's rigctld is restarted without `-P`, so the link is up and PTT is unknown. B accepts. A notes "own rig" and B is told "declined (rig)". The session ID is unchanged and still CONNECTED, and both dials are where check 1 left them. |
| 2 | fallback | When the re-call fails, both stations go back to the original frequency. | B is set Receive only and accepts. Both rigs move to 7 140 000 (B 7 138 500). A's re-calls go unanswered. After the windows, A's dial returns to 7 120 000 and B's to 7 118 500. Both QSY cards clear, and A has no session. |

The checks run in the order 5, 1, 4, 3, 2.

**Check 3 against the plan.** The plan's check 3 stopped A's rigctld before the accept. The design (B3) says a station whose rig link is down takes the manual path ("Tune to … now") and **accepts**, so a stopped rigctld cannot produce a `rig` decline. The lab instead restarts rigctld without `-P`, which is what rigctld does when it can't read PTT. The gate refuses that ("PTT unknown"), as it would a rig stuck in transmit.

**Check 2 against the plan.** The plan said "B is told not to re-call". Only the caller (A) re-calls, so the lab makes A's re-calls fail: B is set Receive only, which means it no longer answers calls.

## Run

Mercury is the pin's patched build (f77adfb + 0001 + 0002), the same build as M3b.1. Outputs go on E:, never F:.

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/src:ro" -v mc-app-build-amd64:/build -v "E:/mc-lab-out/m3c/rig-lab:/out" -v "$PWD/lab/docker/rig:/scripts:ro" mercurychat-app-build:amd64 bash /scripts/build-rig-lab.sh
MSYS_NO_PATHCONV=1 docker run --rm -v "E:/mc-lab-out/m3b1/bin-patched:/mercury:ro" -v "E:/mc-lab-out/m3c/rig-lab:/lab:ro" -v "E:/mc-lab-out/m3c/run-m3c:/out" -v "$PWD/lab/docker/rig:/scripts:ro" mercurychat-lab:rig bash /scripts/run-rig-lab-checks.sh m3c
```

The run writes:
- `report.txt` (the five lines);
- `activity-m3c-qsy.txt` (A) and `activity-m3c-b.txt` (B);
- `lab-manifest.txt`;
- each Mercury's and rigctld's logs.

A run takes about six minutes, most of it check 2's re-call window.

The Mercury contract suite is separate: `lab/docker/contract/check-mercury.sh` (the pin) and `--latest`. See `lab/upstream/M3c-latest.md`.
