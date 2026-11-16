#!/usr/bin/env bash
# Local CI: the same checks GitHub Actions runs. Usage: ./scripts/ci.sh
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check
echo "==> cargo clippy (-D warnings)"
cargo clippy --workspace --all-targets -- -D warnings
echo "==> cargo clippy --features profile (-D warnings)"
cargo clippy -p mv_game --features profile -- -D warnings
echo "==> cargo test"
cargo test --workspace
echo "OK: all checks passed"
