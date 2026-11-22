# Planning changes

## 0.9 — 22 November 2026 (M3c operator extras)

- Built M3c on branch `m3c`. **Mercury contract suite** (`crates/mercury-contract`): every Mercury behaviour MercuryChat relies on, written once. It runs against the fake in `check.sh`, and against real Mercury with `lab/docker/contract/check-mercury.sh`, both at the pin (f77adfb + 0001 + 0002) and at upstream `mercuryv2` (`--latest`).
- **MCH/1 minor 1:** capabilities `qsy/1` and `info/1`, with frames 0x30/0x31 (QSY) and 0x40/0x41 (information) and golden vectors. A new frame is sent only when both sides negotiated it, so an M3b peer chats as before and sees the new controls as "not supported by this station".
- **QSY during a session (D16):** propose 1–5 frequencies from the list. The peer accepts or declines. Each station checks its own rig before anything is committed. The caller disconnects, both rigs are set to the candidate minus their own offset once Mercury is idle and unkeyed, and the caller re-calls; the re-call's QSO record links to the first. If the re-call fails within 120 s, both go back to the original frequency. A station without a rig link can still accept and is shown "Tune to … now".
- **Before a connection:** the QSY button on a heard station sends an addressed MCB/1 kind 3 through the CQ gate. Only the addressee is shown it, with "Tune and listen".
- **Information requests (D17):** "Request SNR" and "Request station info" fill the QSO card, marked "peer report". The fields shared are chosen per profile (name, QTH and locator by default). The email never goes in an answer, only through "Send my email" after a confirmation.
- **Extended beacon (D17):** name, QTH and a status line of up to 40 bytes, as MCB/1 kind 2. Settings → Beacon shows the bytes left at the listen mode, and refuses a beacon that would not fit. A receiver that doesn't know a kind counts it as unsupported, not malformed.
- **Quick messages:** a macro marked "quick" is listed first in the composer's menu.
- **Compatibility:** M3a and M3b receivers count MCB/1 kinds 2 and 3 as **malformed** broadcasts. They predate the "unknown kind is unsupported" rule. They show no station for them and no error, and their malformed counter rises.
- **Rig lab** (`run-rig-lab-checks.sh m3c`, two MercuryChat instances and two patched Mercury modems with Hamlib dummy rigs, no RF): 5 of 5 checks pass:
  - QSY end to end with per-station offsets;
  - the fallback;
  - a failed rig gate declining without a disconnect;
  - SNR and station info with no email;
  - kinds 2 and 3 over the broadcast plane.
- Still open: a real transceiver, the Raspberry Pi smoke test, and on-air timing of the QSY windows. Details: docs/M3c-progress.md.

## 0.8 — 18 November 2026 (M3b rig control)

- Built M3b on branch `m3b`. **Connect-only:** MercuryChat is a second client of the rigctld the operator runs; Mercury keys PTT through the same rigctld (Hamlib model 2). A link test in Settings checks each step (connect, capabilities, frequency, mode, PTT read-back, Mercury's PTT owner) using reads only, and explains rigctld's `RPRT -11` (start it with `-P`). A Mercury with its own CAT connection is refused as a second owner, with the reason.
- **The station strip is the rig panel:** digit tuning (click a digit, scroll or use the arrows, or type), band and mode chips, a frequency-list dropdown, the passband, Tune (first-use confirmation, a 30 s stop), and a popover for VFO/split, RF power and meters. Controls the rig cannot do are hidden. Every change is refused while busy, with the reason shown inline.
- **PTT safety:** MercuryChat never sends `T 1`. After the rig link returns it sends at most one `T 0`, never while Mercury reports PTT on. PTT is cross-checked against Mercury's own PTT events; a contradiction must persist for the whole window before it is a fault, because Mercury unkeys the rig just before it reports PTT off. When PTT cannot be read, only Tune and transmitting actions are refused.
- **Frequency list** shared by every station, with suggested entries per IARU region (drafted, marked for review before release), per-band TX levels per profile, and an optional time-of-day QSY schedule that never runs during a session. Settings is now in tabs (Station, Profiles, Rig, Notifications, Appearance, About), and the chosen tab is remembered.
- The simulator gets a fake rig, so the app and the screenshots show rig control without hardware.
- **Rig lab** (the headless service against two patched Mercury modems keying a Hamlib dummy rig in Docker, no RF): 9 of 9 checks pass. It found one bug, fixed with a test: the PTT cross-check faulted when Mercury unkeyed the rig about 100 ms before its PTT OFF event.
- Still open: the Raspberry Pi smoke test, a real rig on a real serial port, and a review of the suggested frequencies against the current IARU band plans. Next: M3c, operator extras. Details: docs/M3b-progress.md.

## 0.7 — 17 November 2026 (M3a discovery and operator tools)

- Built M3a on branch `m3a`. **Discovery:** the heard list, kept from CQs, MCB/1 beacons and sessions, shows where each SNR, frequency and offset came from, with favourites and a 30-day retention. Malformed and foreign broadcasts are counted, never shown as stations. **CQ and Beacon now** go through an idle-only, capacity-limited broadcast gate, and the result is "transmitted", never "delivered". **Periodic beacons** are off by default and jittered. They are suppressed during calls, sessions, draining, faults and a busy channel, never replayed in a burst, and never re-armed by a restart or recovery.
- Listening is on by default after attach and recovery, with a Receive only switch. Receive only chosen during a session waits for the session to end, because Mercury drops a live link without telling the peer.
- **Telemetry** comes from Mercury's WebSocket. Missing values read "unavailable", never 0, and losing telemetry never ends a session. The **spectrum and waterfall** are optional, collapsed by default, and forwarded at most 5 frames a second, only while shown.
- **Operator tools:** a draft QSO record for every session, edited on a QSO card and kept in a new Logbook (finalise locks a record). Station information and local macros are expanded into the draft, never sent by themselves; the email address goes in only by an explicit "Insert email". There is history search by text or callsign, and inline transcript rows for a session's end (a QSO summary) and for each CQ or beacon transmitted.
- **Window:** the Operate station strip (a seven-segment frequency read-out in DSEG7, band, mode, bandwidth and actions), the Heard tab, telemetry, beacon and activity panels, and an About page ("Founded by ZS6SHZ", licence, credits). About's links open in the system browser, and the app never looks online by itself. Optional **desktop notifications** are off for Virtual Audio Lab stations and support quiet hours. Also: the UI localised in English, Spanish, Portuguese and French (es, pt-BR and fr are machine drafts), theme presets and custom themes, a start-up splash, and remembered window placement.
- **Real-Mercury lab** (two Mercury modems over VB-Audio cables, PTT none) found five modem-behaviour issues. All five were fixed in the app with tests:
  - the CQ/MONITOR line order;
  - LISTEN OFF on a live link;
  - a silently held broadcast port;
  - an SNR shown before any decode;
  - stale beacons sent once a held port frees.
  A Mercury-side fix for the last one went upstream as Rhizomatica/mercury#588. The Hamlib reopen fix (#571) is now merged upstream, so lab builds use upstream `f77adfb7` plus the two still-open patches #565 and #566.
- Still open: the Raspberry Pi smoke test (it gates the exits of M1, M2 and M3a's Pi row), real OS notification toasts checked by a person, and the waterfall drawing real spectra on a release build. Next: M3b, rig control. Details: docs/M3-progress.md.

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
