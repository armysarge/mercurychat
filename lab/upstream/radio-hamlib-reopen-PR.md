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
- **Pace:** at most one reopen every 3 s (`HAMLIB_REOPEN_INTERVAL_MS`). An unreachable server costs a connect timeout under `g_radio_mutex` on the keying path, so it must not be paid on every key-up. `hamlib_close()` clears the pace, so a PTT restart starts fresh.
- **Logging:**
  ```
  [WRN] [radio-io] Hamlib session to 127.0.0.1:4532 lost; reopening
  [INF] [radio-io] Hamlib session reopened
  [WRN] [radio-io] Hamlib reopen failed; next attempt at the next PTT change at least 3000 ms from now
  [WRN] [radio-io] PTT ON not sent: no Hamlib session (reopen pending)
  ```
  The last line is new: with no session, a PTT change used to return without a word, and a lost PTT OFF is the one an operator most needs to see (second commit).

`errno = 0` before Hamlib calls (the 4.6.2 `read_string_generic` workaround) now lives in `hamlib_ptt()`, and the comments that pointed at `hamlib_set` point there.

`.github/workflows/integration.yml`: the Linux integration job installs `libhamlib-utils`, so the new test has `rigctld` and runs in CI instead of skipping.

### What changes for users

- **rigctld restarts while Mercury is idle:** the next key-up reopens the session (0.1–0.3 s here, once) and keys. Nothing else to do.
- **rigctld restarts while Mercury is keyed:** the PTT OFF fails. Once rigctld is back, the next PTT change reopens the session and gets through. With an ARQ session running that is seconds. While idle, the rig is released at the next transmission, not before. A background retry could release it sooner. It is left out here to keep keying on one thread.
- **rigctld stays down:** PTT changes fail as before. A reopen is tried at most every 3 s, and every failure is logged.
- **Serial rigs:** unchanged.

### Not changed here

- **The TNC still reports `PTT ON` when keying failed.** `ptt_on()` in `tcp_interfaces.c` ignores `radio_io_key_on()`'s result. Whether a host should be told, and how (VARA hosts expect `PTT ON` / `PTT OFF`), is a protocol question for its own issue.
- **The failure text:** `rigerror(ret)` returns Hamlib's last debug line, not the error, so a failed PTT logs things like `rig_init: rig has VFO_MAIN_A`. `rigerror2(ret)` would say `IO error`. It is left as it was, so this diff stays about reopening.

## Testing

**`tests/integration/mercury_rigctld_restart_test.go`** (new). It uses one Mercury on `-x null` with `-R 2` and a real `rigctld -m 1 -P RIG`, and is skipped without rigctld or without Hamlib in the build. Each transmission is one broadcast frame, which keys and unkeys once.
1. Transmit: Hamlib's PTT ON and PTT OFF both succeed (rigctld answers `RPRT 0`).
2. Restart rigctld while Mercury is idle, then transmit: the session is reopened, and PTT ON and OFF succeed.
3. Stop rigctld, then transmit: the reopen is tried and fails, the PTT OFF with no session is logged, and Mercury stays up. Start rigctld, wait out the 3 s pace, then transmit: reopened again, and PTT ON and OFF succeed.

| | result |
|---|---|
| New test on 274bd5c (without the change) | **FAIL** at step 2: rig keyed 1/2 and released 1/2 times (PTT ON failed 1, PTT OFF failed 1) |
| New test with the change | **PASS** (22 s) |
| `test_radio_io_frequency`, `test_radio_port` | 5/5, 12/12 |
| Two-station ARQ session over virtual audio, each keying through rigctld, rigctld killed while keyed and restarted (15 checks) | 15/15. The rig is keyed again after the restart without restarting Mercury (before: 0 times). Data, disconnect and PTT correlation (4/4, 10/10) as before |

Built and run on Debian trixie with Hamlib 4.6.2.
