# Mercury contracts against real Mercury

One suite (`crates/mercury-contract`), two targets (M3c design Part A):

- **fake**: `cargo test -p mercury-contract`, part of `scripts/check.sh`;
- **real**: `lab/docker/contract/check-mercury.sh`, two Mercury stations in Docker.

## Running it

From the repository root, in Git Bash on the Windows PC:

```bash
lab/docker/contract/check-mercury.sh             # the pin: lab/mercury-pin.env, with its patches
lab/docker/contract/check-mercury.sh --latest    # the fork's upstream/mercuryv2, no patches
lab/docker/contract/check-mercury.sh --commit <sha> --out E:/mc-lab-out/m4/try-1
```

The fork at `E:/Programming/mercury-fork` is only mounted read-only; nothing is fetched. The steps:

| Step | Script | Image |
|---|---|---|
| Build Mercury at the commit, apply the listed patches | `../build-mercury.sh` | `mercurychat-lab:m0` |
| Build the contract test binary | `build-contracts.sh` | `mercurychat-app-build:amd64` (volume `mc-app-build-amd64`) |
| Run it against stations A and B | `run-contracts.sh` | `mercurychat-lab:rig` |

The stations, both with PTT `none` (no radio, no serial port, no RF):

| | Call | TNC | Broadcast | WebSocket |
|---|---|---|---|---|
| A | N0CTA | 8300/8301 | 8100 | 10000 |
| B | N0CTB | 8400/8401 | 8200 | 10100 |

A rigctld with the Hamlib dummy rig listens on 4532 for the one contract that switches A to Hamlib PTT.

The file contracts (M4, `tests/contracts/files.rs`) run MercuryChat's own session engine on both stations:
`compressed_file_crosses_arq` sends a 16 KiB text file, zstd-encoded, at BW2300 and checks it arrives
byte-exact with its receipt; `chat_during_file_at_bw500` sends a 4 KiB incompressible file at BW500 and a
chat after its first chunk, which must be receipted within 30 s on real Mercury (on the fake, with a 200 B/s
drain, the chat must arrive before the file ends).

## Output

In `E:/mc-lab-out/m4/contracts-<sha7>[-latest]/`:

- `run/summary.txt`: the commit and patches, then `PASS|SKIP|DIVERGENCE|FAIL <contract>`, one line each, and `NOTE <contract> <text>` for a measurement a contract records (M4: `chat_during_file_at_bw500`'s chat receipt latency);
- `run/contracts.log`: the test binary's full output;
- `run/a`, `run/b`: each station's ini and Mercury log;
- `bin/linux-amd64/MANIFEST.txt`, `contracts/MANIFEST.txt`: what was built.

The script exits non-zero on any FAIL.

## When a contract fails on real Mercury (A4)

- **Mercury behaves as intended:** the fake was wrong. Fix the fake, and any app code that relied on it.
- **Mercury looks wrong:** add the contract's name to `divergences.txt` and write `lab/upstream/M3c-<name>.md` (what happens, what should, the evidence). It then reports `DIVERGENCE`; once Mercury is fixed it reports `DIVERGENCE-FIXED` and fails until it is taken off the list.
- **Never** change a contract so that real Mercury passes when the app depends on the old behaviour.

The fake is never bent to match a listed divergence.
