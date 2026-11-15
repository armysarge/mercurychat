# Planning changes

## 0.3 — 15 November 2026 (M0 in progress)

- Implemented the M0 foundation: Rust workspace (`mch-protocol`, `mercury-adapter`, `mc-storage`, `mc-core`, `tools/m0-client`), Tauri 2 + Vue 3 shell (`apps/desktop`, `src-tauri`), SQLite schema v1, MCH/1 golden vectors.
- Built Mercury 1.9.24 (274bd5c9) for Windows x64, Linux amd64 and arm64 in Docker; built MercuryChat for Windows (NSIS), Debian amd64 and arm64 (.deb).
- Linux crossed-audio lab with two real Mercury instances: M0 scenario 17/17 PASS. Found and fixed recovery issues (idle data queued into next session; ABORT teardown cancelled by early LISTEN OFF).
- Raspberry Pi kit prepared; Pi gate still open. Details: docs/M0-progress.md.
- Reviewed owner-supplied VarAC screenshots as a workflow checklist (docs/04 §8); owner decided Core includes rig control through one shared rigctld (D13). Dummy-rig lab 14/14.
- Windows virtual-cable lab over VB-Audio Cable A/B: M0 scenario 17/17 at BW2300 and BW500; desktop shell alongside both modems with no measurable effect.
- Proved CQ and MCB/1 beacons over Mercury's broadcast plane (11/11): KISS codec, MCB/1 encoder/strict decoder with golden vectors, idle-only and capacity-limited broadcast gate.

## 0.2 — 14 November 2026

- Confirmed virtual audio cables as the initial test arrangement; specified two independent crossed routes, isolated profiles/endpoints and PTT none.
- Made Raspberry Pi ARM64 a Core release gate, covering a local desktop station and a remote modem-only role.
- Added separate Windows, Ubuntu, Debian and Pi qualification rows, CPU/ABI packaging checks, compact UI and actual-device resource/thermal/audio tests.
- Revised provisional effort from 10–18 to 12–20 developer-weeks; re-estimation remains an M0 outcome.
- No production app, radio setup, driver installation or compatibility proof was performed.

## 0.1 — 14 November 2026

Initial product, architecture, proposed protocol, UI concept, roadmap and evidence baseline.
