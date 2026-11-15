A related gap that this PR leaves alone, raised here before I send anything for it: when keying fails, the host is still told that it worked.

`ptt_on()` in `data_interfaces/tcp_interfaces.c` ignores the result of `radio_io_key_on()`, queues `PTT ON` and the modem modulates anyway. In the lab run in the description, after the rigctld restart Mercury sent `PTT ON` once and the rig was keyed 0 times. On air, the peer hears nothing and the host has no way to tell.

`PTT ON` / `PTT OFF` should keep meaning "Mercury is transmitting", since hosts that key their own radio act on them. So I'd leave those lines as they are and add the failure next to them. The options I see:

1. **A new control-port line** when Mercury's own keying fails, e.g. `PTT FAILED`, sent after `PTT ON`. Hosts that don't know it should ignore it, but I'd want your view on VARA hosts meeting an unknown line.
2. **UI and log only:** a PTT-error field in the WebSocket status, plus the existing warning. No TNC protocol change.
3. **Don't modulate into an unkeyed rig**, i.e. skip the transmission when keying fails. This touches ARQ timing, so it's probably not worth it.

Which would you take? I can do it as a follow-up PR.
