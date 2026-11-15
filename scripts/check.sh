#!/usr/bin/env bash
# The gate every change passes before it is committed (docs/DEVELOPING.md).
# CI runs the same script.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix apps/desktop run build

# The UI's TypeScript contract is generated from the Rust types by `cargo test`
# (ts-rs); what is committed must match what the code generates.
b=apps/desktop/src/bindings
if ! git diff --quiet -- "$b" || [ -n "$(git ls-files --others --exclude-standard -- "$b")" ]; then
  echo "UI bindings in $b are out of date: commit what cargo test generated" >&2
  exit 1
fi
