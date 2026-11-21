# M3b.1 against upstream Mercury: no divergence

**Checked:** 21 Nov 2026.

**Upstream build:** Mercury f77adfb76f10e9380fc8fb9c2bfc8da889538235 ("Merge pull request #583", `mercuryv2`), built for linux-amd64 with no patches. Its sha256 is `7211c496…07cb`.

**Patched build:** the same commit plus 0001 (#565, bind_address) and 0002 (#566, WebSocket Origin/Host checks).

**How it was run:** `run-rig-lab-checks.sh m3b1`, the same run on each build (see `lab/docker/rig/M3b1-checks.md`).

The upstream build passed all seven checks (7 of 7), the same as the patched build. This includes the two that check 7 names:

- **3, PTT push:**
  - Upstream's `set_ptt_config` accepted method `hamlib`, device `127.0.0.1:4532` and model `2`.
  - It answered with a `radio_list` echo: `ptt_method hamlib`, `device_path 127.0.0.1:4532`, `ptt_line rts`, `ptt_invert none`, `cm108_gpio 3`.
  - Mercury then keyed the dummy rig through MercuryChat's rigctld during a session: a separate `rigctl … t` read `1` 55 times.
- **4, audio:**
  - Upstream's `set_audio_config` applied `a_to_b.monitor` and `b_to_a` (`audio_ok: true`) and saved them to `mercury.ini`.
  - A bogus input gave `audio_ok: false` with no `audio_error` text, and left the ini unchanged.

Seen on both builds and not a divergence: with the bogus device, Mercury's status reports `audio_ok: false` but no `audio_error`. MercuryChat shows "?" as the reason. If Mercury sent the error text, the Audio tab could show it. That would be a small upstream improvement, not filed.
