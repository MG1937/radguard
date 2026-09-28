$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

Write-Host "==> Rust unit and integration tests"
cargo test --all-targets --locked
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "==> Clippy"
cargo clippy --all-targets --locked -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "==> Release build"
cargo build --release --locked
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$RustBin = if ($IsWindows -or $env:OS -eq "Windows_NT") {
    "target/release/radguard.exe"
} else {
    "target/release/radguard"
}

Write-Host "==> Frozen DAD quality gates (Phase 1 = 0, Phase 2 <= 70)"
python -S tests/quality_gate.py --phase 2 --no-build --rust-bin $RustBin
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "==> All gates passed"
