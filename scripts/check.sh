#!/usr/bin/env bash
# The gate every change passes before it is committed (docs/DEVELOPING.md).
# CI runs the same script.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
# Test hooks must never reach a shipped crate (they carry an arbitrary-payload path).
if grep -rn "test-hooks" src-tauri/Cargo.toml tools/*/Cargo.toml; then
  echo "test-hooks must not be enabled outside mc-core's own dev-dependencies" >&2
  exit 1
fi
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --features mc-core/sim
# The workspace build turns the simulator on (src-tauri enables it); check the
# core without it too, so the cfg(not(feature = "sim")) path keeps compiling.
cargo clippy -p mc-core --all-targets -- -D warnings
node apps/desktop/scripts/check-i18n.mjs
# Themes: contrast (WCAG AA/AAA), no colour literals outside the theme files, apply.ts unit tests.
node apps/desktop/scripts/check-contrast.mjs
node apps/desktop/scripts/check-colours.mjs --self-test
node apps/desktop/scripts/check-colours.mjs
node --test apps/desktop/scripts/apply.test.mjs
# Desktop notifications: the pure decision (notify.ts).
node --test apps/desktop/scripts/notify.test.mjs
# The station strip's tuning arithmetic (freqStep.ts).
node --test apps/desktop/scripts/freqStep.test.mjs
# Settings → Rig: which boxes show, which PTT choices fit (rigForm.ts).
node --test apps/desktop/scripts/rigForm.test.mjs
# Quick messages first in the composer's macro menu (quick.ts).
node --test apps/desktop/scripts/quick.test.mjs
npm --prefix apps/desktop run build

# The UI's TypeScript contract is generated from the Rust types by `cargo test`
# (ts-rs); what is committed must match what the code generates.
b=apps/desktop/src/bindings
if ! git diff --quiet -- "$b" || [ -n "$(git ls-files --others --exclude-standard -- "$b")" ]; then
  echo "UI bindings in $b are out of date: commit what cargo test generated" >&2
  exit 1
fi
