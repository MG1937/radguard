# Build RadGuard on Linux

## Requirements

- A recent stable Rust toolchain with Cargo (the project uses Rust edition 2024)
- A C compiler and build tools, required by `mimalloc`
- Internet access to crates.io for the first build

Debian/Ubuntu example:

```bash
sudo apt update
sudo apt install -y build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup update stable
```

## Build

From the extracted source directory:

```bash
cargo build --release --locked
```

The executable will be:

```text
target/release/radguard
```

## Usage examples

```bash
# Inspect a DEX
./target/release/radguard inspect classes.dex

# Decompile one class
./target/release/radguard decompile-dad classes.dex 'Lcom/example/MainActivity;'

# Export an APK to package directories and individual .java files
./target/release/radguard export-all app.apk output

# Parallel export, automatically using all available logical CPUs
./target/release/radguard export-all-parallel app.apk output

# Parallel export with an explicit worker count
./target/release/radguard export-all-parallel app.apk 8 output
```

Without an output directory, `export-all` and `export-all-parallel` write the binary `RGOUT001` stream to stdout. Redirect it to a file rather than a terminal:

```bash
./target/release/radguard export-all app.apk > app.rgout
./target/release/radguard export-all-parallel app.apk > app.rgout
```

## Checks

Run every gate locally with the same script used by GitHub Actions:

```bash
bash scripts/check-all.sh
```

Or run each stage manually:

```bash
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
python3 -S tests/quality_gate.py --phase 2 --no-build --rust-bin target/release/radguard
```

The Python quality gate uses only the checked-in frozen `pickle.gz` DAD cache and its frozen
comparison oracles. It does not run Androguard or regenerate reference output.
