#!/usr/bin/env bash
# The gate every change passes before it is committed (docs/DEVELOPING.md).
# CI runs the same script.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm --prefix apps/desktop run build
