# Planning changes

## 0.6 — 15 November 2026 (M2 connected messaging)

- Built M2 on branch `m2-chat`: one modem task per station that owns Mercury; explicit listening that is off after every start and recovery (RF-02); calls with cancel, disconnect (drain) and abort; MCH/1 negotiation that names a non-MercuryChat peer instead of failing silently; a durable outbox with per-peer quotas, drafts and paged history; end-to-end receipts only after the receiver has stored the message; duplicates shown once; untrusted peer callsigns and control lines validated and bounded.
- A dropped link or an abort marks every unconfirmed message "uncertain" with a Retry that reuses the same ID; restart and recovery never resend on their own.
- The chat workspace replaces the M1 test window: first-use setup, conversation list, transcript with delivery states in words, composer with a byte count, session pane, recovery banners, and drawers down to 800×480. The window draws its own title bar and thin, themed scrollbars.
- Labs over real Mercury with PTT none: Windows app ↔ Mercury in the Linux container, and Windows ↔ Windows over VB-Audio cables, each passing attach, call, Unicode chat (sha256 equal at the peer), stop mid-message → uncertain, restart with nothing resent, and Retry delivered once. These found two bugs, both fixed with tests: the HELLO wait was too short for a real modem (now 120 s), and a late command reply could overwrite newer message states in the window.
- CI green on Windows, Ubuntu x64 and Ubuntu arm64; clean Windows, Debian amd64 and Debian arm64 builds. Still open: the Raspberry Pi smoke test, which gates the exit of M1 and M2. Estimates unchanged. Details: docs/M2-progress.md.

## 0.5 — 15 November 2026 (M1 foundation)

- Built M1's foundation on branch `m1-foundation`: one check gate (`scripts/check.sh`, also CI); data-root and endpoint locks (MOD-02); schema v2 migrations with a backup taken first; station profiles with validation; MOD-01 modem states and a typed command/event contract exported to TypeScript; daily logs kept 7 days or 25 MiB, plus crash reports; the application service actor; and an in-process simulator modem.
- The desktop window now uses only typed commands (`app_call`, `events_since`, the `mc://event` stream) with an explicit command allowlist. A second instance on the same data root shows why it can't start. The selected profile, and its simulator, come back after a restart. All three were checked in the real window by `lab/windows/m1-ui-check.mjs`.
- The amd64 `.deb` installs and runs on bare Ubuntu 24.04. Windows, Linux amd64 and Linux arm64 packages build from the branch.
- Still open before M1 exits: the Raspberry Pi smoke test, then the first-run wizard, navigation and themes (Task 11). Estimates unchanged. Details: docs/M1-progress.md.

## 0.4 — 15 November 2026 (M1 started)

- Closed M0 with the Raspberry Pi run as an exception: go for M1 on Tauri/Vue/Rust, on condition that the Pi smoke test passes before M1's UI work. Re-estimated to 14–21.5 developer-weeks (12–18.5 to go). See docs/M0-go-no-go.md.

## 0.3 — 15 November 2026 (M0)

- Implemented the M0 foundation: Rust workspace (`mch-protocol`, `mercury-adapter`, `mc-storage`, `mc-core`, `tools/m0-client`), Tauri 2 + Vue 3 shell (`apps/desktop`, `src-tauri`), SQLite schema v1, MCH/1 golden vectors.
- Built Mercury 1.9.24 (274bd5c9) for Windows x64, Linux amd64 and arm64 in Docker; built MercuryChat for Windows (NSIS), Debian amd64 and arm64 (.deb).
- Linux crossed-audio lab with two real Mercury instances: M0 scenario 17/17 PASS. Found and fixed recovery issues (idle data queued into next session; ABORT teardown cancelled by early LISTEN OFF).
- Raspberry Pi kit prepared; Pi gate still open. Details: docs/M0-progress.md.
- Reviewed owner-supplied VarAC screenshots as a workflow checklist (docs/04 §8); owner decided Core includes rig control through one shared rigctld (D13). Dummy-rig lab 14/14.
- Windows virtual-cable lab over VB-Audio Cable A/B: M0 scenario 17/17 at BW2300 and BW500; desktop shell alongside both modems with no measurable effect.
- Closed Mercury's listener exposure with upstream PR #565 (`bind_address`): loopback-only binding proven, LAN refused, scenario 17/17; patch mechanism in `lab/mercury-patches/`.
- Closed browser access to Mercury's UI WebSocket with upstream PR #566 (Origin/Host checks), merged onto #565: cross-origin, `null` and DNS-rebinding upgrades refused (36/36), scenario 17/17; managed mode writes `ui_allowed_origins = ""`.
- Wrote the upstream fix for Mercury's dead Hamlib session after a rigctld restart (fork branch `radio-hamlib-reopen`): new integration test fails before and passes after; rig lab 15/15 with keying restored without a Mercury restart. Sent upstream as PR #571 and adopted as patch `0003` (rig lab 15/15 on 274bd5c + 0001–0003); PR text in `lab/upstream/`.
- Proved CQ and MCB/1 beacons over Mercury's broadcast plane (11/11): KISS codec, MCB/1 encoder/strict decoder with golden vectors, idle-only and capacity-limited broadcast gate.

## 0.2 — 14 November 2026

- Confirmed virtual audio cables as the initial test arrangement; specified two independent crossed routes, isolated profiles/endpoints and PTT none.
- Made Raspberry Pi ARM64 a Core release gate, covering a local desktop station and a remote modem-only role.
- Added separate Windows, Ubuntu, Debian and Pi qualification rows, CPU/ABI packaging checks, compact UI and actual-device resource/thermal/audio tests.
- Revised provisional effort from 10–18 to 12–20 developer-weeks; re-estimation remains an M0 outcome.
- No production app, radio setup, driver installation or compatibility proof was performed.

## 0.1 — 14 November 2026

Initial product, architecture, proposed protocol, UI concept, roadmap and evidence baseline.
