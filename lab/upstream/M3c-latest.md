# M3c contracts against latest upstream Mercury: no divergence

**Checked:** 22 Nov 2026, with `lab/docker/contract/check-mercury.sh --latest`. This uses the fork's `upstream/mercuryv2` as it was locally, with no network fetch.

**Latest build:**
- Mercury 33efac4e3089f64f493fde1232b8e96305d15185 ("Updates for release 1.9.25", 21 Nov 2026), built for linux-amd64 with no patches.
- sha256 `037dfef4…292a`.
- 191 commits past the pin, including 46 merged PRs. Among them are #565 (TNC bind address, our patch 0001), #632, #638 and #640 (ARQ call and abort fixes).

**Pinned build:** f77adfb plus 0001 (#565) and 0002 (#566, WebSocket Origin/Host checks). Its sha256 is `15b1edcc…57ad`, the same build as M3b.1.

| Build | Result |
| --- | --- |
| Pin (f77adfb + 0001 + 0002) | 33 PASS, 1 SKIP, 0 FAIL |
| Latest (33efac4, no patches) | 33 PASS, 1 SKIP, 0 FAIL |

- The one SKIP is `unknown_control_line_tolerated`. It is tagged fake-only, because real Mercury can't be made to send an unknown control line.
- No contract is tagged `known-divergence`, and none is needed at either commit.
- The M3c contracts passed on both builds:
  - `new_frames_cross_arq_unchanged`: 0x30, 0x31, 0x40 and 0x41 over a real ARQ link;
  - `kind2_at_capacity_crosses`, `kind2_one_over_refused` and `kind3_max_crosses`: MCB/1 kinds 2 and 3 on the broadcast plane.

**What this means for the pin:**
- Moving the pin to 1.9.25 would drop patch 0001, since #565 is merged.
- 0002 (#566) is not merged upstream. The suite passes without it, so whether to keep it is a security choice (the WebSocket Origin/Host checks), not a compatibility one.
- Nothing in MercuryChat has to change for 1.9.25.
- Moving the pin is the owner's call. It isn't done here.

Outputs:
- `E:/mc-lab-out/m3c/contracts-f77adfb/run/summary.txt`
- `E:/mc-lab-out/m3c/contracts-33efac4-latest/run/summary.txt`
- the build and run logs beside each summary.
