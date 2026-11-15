## Summary

When Mercury keys through rigctld (`-R 2`, NET rigctl) and rigctld restarts, Mercury now reopens its Hamlib session at the next PTT change and keys as before. Until now every later PTT call failed until Mercury itself was restarted, while the TNC kept telling the host `PTT ON` / `PTT OFF`. A PTT OFF lost that way leaves a real rig keyed.

Only network rigs are reopened (rigctld, FLRig, FlexRadio and the other `RIG_PORT_NETWORK` models). Serial rigs are unchanged.

## The problem

### What happened

Two Mercury 1.9.24 (274bd5c9) stations on Linux, each keying a Hamlib dummy rig through its own `rigctld 4.6.2 -m 1 -P RIG`. Mercury runs with `-R 2 -A 127.0.0.1:4532`. An ARQ session is running, and rigctld is killed while station A is keyed, then started again on the same port 15 s later:

```
                       rigctld killed at +38.1 s, A keyed
[+38.510s] [WRN] [radio-io] PTT OFF failed (Hamlib model 2): write_block(): TX 4 bytes
[+44.351s] [WRN] [radio-io] PTT ON failed (Hamlib model 2): write_block(): TX 4 bytes
                       rigctld running again from +53.1 s
[+55.062s] [WRN] [radio-io] PTT OFF failed (Hamlib model 2): netrigctl_transaction: called len=4
[+60.898s] [WRN] [radio-io] PTT ON failed (Hamlib model 2): netrigctl_transaction: called len=4
[+68.109s] [WRN] [radio-io] PTT OFF failed (Hamlib model 2): netrigctl_transaction: called len=4
[+73.706s] [WRN] [radio-io] PTT ON failed (Hamlib model 2): write_block():1135 failed -1 - Broken pipe
[+77.417s] [WRN] [radio-io] PTT OFF failed (Hamlib model 2): netrigctl_transaction: called len=4
```

Hamlib's own trace says `rig_set_ptt returning(-6) IO error` for each one. After the restart, Mercury sent `PTT ON` to the host once and the rig was keyed 0 times. Restarting Mercury was the only way back: it then keyed at once.

With a real transceiver behind rigctld, the PTT OFF at the moment of the kill is lost, and the rig keeps transmitting until its own timeout or the next PTT change that reaches it, and with a dead session no PTT change ever does.

rigctld restarts for ordinary reasons: an update, a crash, or a rig-control app that owns rigctld restarting it.

### The code

`radio_io/radio_io.c`, `hamlib_set()`: a failed `rig_set_ptt` is logged and returns -1. Nothing closes or reopens `g_radio`, so the dead socket is used for every call after it. Hamlib's netrigctl does not reconnect on its own (4.6.2).

## The change

`radio_io/radio_io.c`:
- **`hamlib_reopen()`:** closes the dead session (`rig_close`, `rig_cleanup`) and opens a new one through `hamlib_open()` with the same config, so the open path, port checks and NetRigCtl VFO option are the existing ones. Only for network rigs (`radio_io_port_kind() == RADIO_PORT_KIND_NETWORK`): a serial open can take seconds on the keying path, and a USB fd handed over by the UI cannot be opened a second time.
- **`hamlib_set()`:** on `-RIG_EIO`, reopen and retry that one PTT call. If the reopen fails, `g_radio` is NULL until a later PTT change reopens it.
- **Pace:** key-ups make at most one reopen every 3 s (`HAMLIB_REOPEN_INTERVAL_MS`). An unreachable server costs a connect timeout under `g_radio_mutex` on the keying path, so it must not be paid on every key-up. A PTT OFF is not paced: a late unkey costs a carrier left on air, and a refused connect to a local server is immediate. `hamlib_close()` clears the pace, so a PTT restart starts fresh.
- **A lost PTT OFF is retried** (third commit). `g_hamlib_off_pending` is set when an OFF is not delivered and cleared when any PTT change reaches the rig. While it is set, the OFF is retried every 5 s with a paced reopen, under `g_radio_mutex` via `trylock`, and never while keyed. The retry runs from a new `radio_io_service()`, which `main()`'s idle loop calls, so it works without `-G`. It also runs from the UI frequency poll, for hosts that drive the engine without `main()`. The iOS/host `radio_host.c` gets a no-op.
- **Logging:**
  ```
  [WRN] [radio-io] Hamlib session to 127.0.0.1:4532 lost; reopening
  [INF] [radio-io] Hamlib session reopened
  [WRN] [radio-io] Hamlib reopen failed; next attempt at the next PTT change, or within 5000 ms while a PTT OFF is pending
  [WRN] [radio-io] PTT ON not sent: no Hamlib session (reopen pending)
  [INF] [radio-io] Pending PTT OFF delivered: the rig is released
  ```
  The last line is new: with no session, a PTT change used to return without a word, and a lost PTT OFF is the one an operator most needs to see (second commit).

`errno = 0` before Hamlib calls (the 4.6.2 `read_string_generic` workaround) now lives in `hamlib_ptt()`, and the comments that pointed at `hamlib_set` point there.

`.github/workflows/integration.yml`: the Linux integration job installs `libhamlib-utils`, so the new test has `rigctld` and runs in CI instead of skipping.

### What changes for users

- **rigctld restarts while Mercury is idle:** the next key-up reopens the session (0.1–0.3 s here, once) and keys. Nothing else to do.
- **rigctld restarts while Mercury is keyed:** the PTT OFF fails and is remembered. Once rigctld is back, the idle retry reopens the session and releases the rig within about 5 s, with no new transmission (5.3 s in the test). A PTT change in the meantime gets through too.
- **rigctld stays down:** PTT changes fail as before. A key-up tries a reopen at most every 3 s, an unkey always tries, a pending OFF is retried every 5 s, and every failure is logged.
- **Serial rigs:** unchanged.

### Not changed here

- **The TNC still reports `PTT ON` when keying failed.** `ptt_on()` in `tcp_interfaces.c` ignores `radio_io_key_on()`'s result. Whether a host should be told, and how (VARA hosts expect `PTT ON` / `PTT OFF`), is a protocol question for its own issue.
- **The failure text:** `rigerror(ret)` returns Hamlib's last debug line, not the error, so a failed PTT logs things like `rig_init: rig has VFO_MAIN_A`. `rigerror2(ret)` would say `IO error`. It is left as it was, so this diff stays about reopening.

## Testing

**`tests/integration/mercury_rigctld_restart_test.go`** (new). Each test uses one Mercury on `-x null` with `-R 2` and a real `rigctld -m 1 -P RIG`, and is skipped without rigctld or without Hamlib in the build. Each transmission is one broadcast frame, which keys and unkeys once.
1. Transmit: Hamlib's PTT ON and PTT OFF both succeed (rigctld answers `RPRT 0`).
2. Restart rigctld while Mercury is idle, then transmit: the session is reopened, and PTT ON and OFF succeed.
3. Stop rigctld, then transmit: the reopen is tried and fails, the PTT OFF with no session is logged, and Mercury stays up. Start rigctld and send nothing: the pending OFF is retried, the session reopened, and the rig released. Then transmit: PTT ON and OFF succeed.

`TestHamlibPttOffIsNotPaced` uses DATAC0 (about 1.1 s on air), so the unkey lands inside the 3 s pace after a failed key-up reopen. It checks that the unkey makes its own reopen attempt. A DATAC3 frame lasts about 3.9 s, longer than the pace, so the first test cannot see this.

| | result |
|---|---|
| New test on 274bd5c (without the change) | **FAIL** at step 2: rig keyed 1/2 and released 1/2 times (PTT ON failed 1, PTT OFF failed 1) |
| New test with the change | **PASS** (22 s) |
| Both tests on `5e5d7ad` (the PR before the review follow-up) | **FAIL**: the pending OFF is never sent; the unkey is dropped by the pace (1 failed reopen, want 2) |
| Both tests with the follow-up (`940b946`) | **PASS** (23 s, 3 s) |
| `TestMercuryTxTest*` (shutdown and forced-exit unkey) | 3/3 before and after |
| `test_radio_io_frequency`, `test_radio_port`, `test_host` | 5/5, 12/12, 6/6 |
| Two-station ARQ session over virtual audio, each keying through rigctld, rigctld killed while keyed and restarted (15 checks) | 15/15. The rig is keyed again after the restart without restarting Mercury (before: 0 times). Data, disconnect and PTT correlation (4/4, 10/10) as before |

Built and run on Debian trixie with Hamlib 4.6.2.
