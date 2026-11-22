# M4: the Mercury pin moves to 1.9.25

**Done:** 22 Nov 2026 (M4 Task 1). **Pin:** `33efac4e3089f64f493fde1232b8e96305d15185` ("Updates for release 1.9.25"), the newest `upstream/mercuryv2` in the local fork. There was no fetch.

**Patches** (`lab/mercury-patches/`, applied in order):
- `0001-pr565-bind-address.patch`: the `bind_address` setting.
- `0002-pr566-ws-origin-host.patch`: the WebSocket Origin/Host checks (`ui_allowed_origins`), on top of 0001.
- `0003` (the Hamlib reopen fix, PR #571) is deleted. It has been upstream since f77adfb.

Both remaining patches were regenerated in a container from the f77adfb versions:
- 0001 had two conflicts:
  - `tcp_interfaces.c`: kept upstream's newer bind-failure log line;
  - the test runner's list: kept both sets of tests.
- 0002 then cherry-picked cleanly.
- Mercury builds with both patches and its own `make test` passes.

## What M3c got wrong about upstream

[M3c-latest.md](M3c-latest.md) said #565 was merged and 0001 could be dropped. That is not so.

**#565 (`bind_address`):**
- The maintainer asked for the setting to be removed. The PR was merged (fa892f9c) without it: only bind-error logging and a README "Restricting network access" section with an nftables rule.
- 1.9.25 still listens on `INADDR_ANY`.

**#566 (WebSocket Origin/Host checks):** closed, not merged. The maintainer's reason: the UI WebSocket accepting any origin is policy, because the hosted web app connects to it from another origin.

**Upstream policy** (Mercury's SECURITY.md, #569): Mercury's ports are open on every interface by design, and access is restricted with a host firewall. Neither patch will be accepted upstream, so MercuryChat carries both.

**For M5 (managed Mercury):** this is the owner's decision to make. MercuryChat can either:
- ship a patched Mercury that listens only on 127.0.0.1, and keep carrying these two patches; or
- ship stock Mercury and add a firewall rule, or warn the operator.

A station's own stock Mercury ("existing Mercury" profile) is open on the network either way, and Settings should say so.

## Changes MercuryChat needed for 1.9.25

- **Listen-mode frame sizes changed:**
  - QAM16C2 is 1183 bytes (it was 1213);
  - DATAC17 is 1184 bytes (it was 1180).

  The tables in `mercury-adapter/src/kiss.rs` and `mch-protocol/src/beacon.rs` are updated. The modes 1.9.25 adds (13–18) aren't selectable listen modes in MercuryChat. M3c's "no divergence" run missed this, because no contract uses those two modes.
- **New fixture and checks:**
  - `tests/fixtures/mercury-listen-modes.txt` holds `mercury -l` at the pin.
  - A unit test checks `kiss.rs` against the fixture, and the existing `capacity_tables_match` checks `beacon.rs` against `kiss.rs`.
  - `check-mercury.sh` step 1b diffs every build's `mercury -l` against the fixture. A difference fails a pin run, and is only reported for `--latest` and `--commit` runs.
- **VERSION** now answers with two lines: `VARA version 4.9.0 registered`, then `VERSION VARA HF v4.9.0` (for Pat, #638). Both parse as the version answer, and the fake sends both.
- No MercuryChat code checks Mercury's version number.

## Results on the new pin

| Check | Result | Output |
|---|---|---|
| Contract suite (`check-mercury.sh`) | 33 PASS, 1 SKIP (`unknown_control_line_tolerated`, fake-only), 0 FAIL | `E:/mc-lab-out/m4/contracts-33efac4-pin/` |
| Rig lab `m3c` | 5 of 5 (see the note below) | `E:/mc-lab-out/m4/run-m3c`, `run-m3c-quiet` |
| Rig lab `m3b1` | 7 of 7 | `E:/mc-lab-out/m4/run-m3b1` |
| Rig lab `m3b` | 9 of 9 | `E:/mc-lab-out/m4/run-m3b` |

**Build:** linux-amd64, sha256 `fb6f6f7526fd7bebfb40bd3db12860a90f5ac15ef3e28633bd9e55495709c1a2`. Its `MANIFEST.txt` lists 0001@b674e08fa75f and 0002@d3720e1eb833.

**The `m3c` check 3 note:**
- The first run failed check 3 (4 of 5). The lab had restarted A's rigctld while A was transmitting. A's frame was cut off on the air.
- Mercury's ARQ then stalled for about 40 s with neither side keying. B's accept missed A's 60 s answer window, so A timed out instead of failing its gate.
- MercuryChat was correct throughout: no disconnect, no retune.
- An unchanged rerun passed 5 of 5. The lab now restarts the rigctld only after A has been unkeyed for 3 s.
- **Upstream observation:** after one truncated frame, 1.9.25 took about 40 s to resume. The same check had passed at f77adfb. It's worth watching on air in M6.
