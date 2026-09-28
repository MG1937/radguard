#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo '==> Rust unit and integration tests'
cargo test --all-targets --locked

echo '==> Clippy'
cargo clippy --all-targets --locked -- -D warnings

echo '==> Release build'
cargo build --release --locked

if python3 -S -c 'import sys' >/dev/null 2>&1; then
  python_cmd=(python3)
elif py -3 -S -c 'import sys' >/dev/null 2>&1; then
  python_cmd=(py -3)
else
  echo 'Python 3 is required for the frozen DAD quality gate' >&2
  exit 1
fi

rust_bin=target/release/radguard
if [[ -x target/release/radguard.exe ]]; then
  rust_bin=target/release/radguard.exe
fi

echo '==> Frozen DAD quality gates (Phase 1 = 0, Phase 2 <= 70)'
"${python_cmd[@]}" -S tests/quality_gate.py \
  --phase 2 \
  --diff-workers "${RADGUARD_DIFF_WORKERS:-$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)}" \
  --no-build \
  --rust-bin "$rust_bin"

echo '==> All gates passed'
