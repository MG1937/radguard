# RadGuard

RadGuard is a high-performance Android DEX/APK decompiler written in Rust. It reconstructs
Java-like pseudocode from Dalvik bytecode and supports both single-threaded DEX processing and
true APK-wide parallel decompilation across multiple `classes*.dex` files.

The decompilation pipeline is a Rust rewrite based on the design and behavior of the DAD
(Dalvik Analysis Decompiler) engine in [Androguard](https://github.com/androguard/androguard).
RadGuard reimplements DEX parsing, instruction lowering, intermediate representation, data-flow
analysis, control-flow structuring, and source rendering in Rust.

> RadGuard produces reconstructed Java-like pseudocode. Decompiled output is intended for
> inspection and analysis and is not guaranteed to compile as Java source without manual changes.

## Acknowledgements

RadGuard would not exist without the work of the Androguard contributors and the original DAD
engine authors. The DAD implementation provided the behavioral reference and architecture used
while building this Rust rewrite.

Special thanks to:

- [Androguard](https://github.com/androguard/androguard)
- The authors and maintainers of Androguard's DAD decompiler
- The wider Android reverse-engineering community

RadGuard is an independent Rust implementation. It does not invoke Python or embed Androguard at
runtime.

## Features

- Pure Rust DEX parser and Dalvik instruction decoder
- DAD-inspired IR construction and opcode lowering
- Def-use/use-def analysis, propagation, and declaration placement
- Control-flow graph analysis and Java-like structure recovery
- Exception-handler and catch-region reconstruction
- Java-like pseudocode with annotations enabled by default
- One `.java` output file per class, organized by package
- Direct DEX and multidex APK support
- True APK-wide parallelism across `classes.dex`, `classes2.dex`, and later DEX files
- Class-level parallelism for single-DEX inputs
- Direct ZIP Central Directory probing without scanning or extracting unrelated APK entries
- Parallel Deflate decompression through `libdeflate`
- Live APK/DEX progress reporting
- Public Rust API for embedding RadGuard in other projects
- Optional annotation-free DAD-compatible output

## Build

RadGuard uses Rust edition 2024. Install a recent stable Rust toolchain and a C compiler, then run:

```bash
cargo build --release --locked
```

The executable is written to:

```text
target/release/radguard
```

On Debian or Ubuntu:

```bash
sudo apt update
sudo apt install -y build-essential curl
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup update stable
cargo build --release --locked
```

See [LINUX_BUILD.md](LINUX_BUILD.md) for additional Linux instructions.

## Command-line usage

### Inspect a DEX

```bash
radguard inspect classes.dex
```

This prints the DEX version and table, class, field, and method counts without decompiling it.

### Decompile one class

Normal annotated output:

```bash
radguard decompile classes.dex 'Lcom/example/MainActivity;'
```

Annotation-free DAD-compatible output:

```bash
radguard decompile-dad classes.dex 'Lcom/example/MainActivity;'
```

### Export a DEX or APK as Java files

Single-threaded:

```bash
radguard export-all app.apk output
```

Parallel, automatically using all logical CPUs available to the process:

```bash
radguard export-all-parallel app.apk output
```

Parallel with an explicit global worker count:

```bash
radguard export-all-parallel app.apk 8 output
```

The output follows the reconstructed package hierarchy:

```text
output/
└── com/
    └── example/
        ├── MainActivity.java
        └── MainActivity$Inner.java
```

For multidex APKs, workers concurrently read, decompress, parse, and decompile different DEX
files. Only exact root entries named `classes.dex` or `classesN.dex` are selected. Unrelated APK
resources are not extracted or decompressed.

### Binary aggregate output

If no output directory is supplied, `export-all` and `export-all-parallel` write the binary
`RGOUT001` stream to standard output. Redirect it to a file rather than a terminal:

```bash
radguard export-all app.apk > app.rgout
radguard export-all-parallel app.apk 16 > app.rgout
```

Progress is written to standard error and does not corrupt the binary stream.

## Rust library API

Add RadGuard as a path dependency:

```toml
[dependencies]
radguard = { path = "../radguard" }
```

Export an APK with annotations and all available logical CPUs:

```rust
use radguard::cli::{ExportOptions, export_to_directory};

fn main() -> radguard::Result<()> {
    let summary = export_to_directory(
        "app.apk",
        "output",
        ExportOptions::parallel(),
    )?;

    println!(
        "exported {} classes from {} DEX files in {:?}",
        summary.classes,
        summary.dex_files,
        summary.duration,
    );
    Ok(())
}
```

Normal exports include annotations by default. Historical annotation-free DAD rendering can be
requested explicitly:

```rust
let options = ExportOptions::parallel().dad_compatible();
```

See [docs/RUST_API.md](docs/RUST_API.md) for the complete embedding guide, progress callbacks,
single-class APIs, and `RGOUT001` writer integration.

## Architecture

```text
APK Central Directory probe
        │
        ├── classes.dex ── libdeflate ── DEX parser ── decompiler worker
        ├── classes2.dex ─ libdeflate ── DEX parser ── decompiler worker
        ├── classes3.dex ─ libdeflate ── DEX parser ── decompiler worker
        └── ...
                                      │
                                      ▼
                          ordered Java/source output
```

The main decompilation stages are:

```text
DEX parsing
  → Dalvik instruction decoding
  → opcode lowering
  → DAD-style intermediate representation
  → data-flow optimization
  → CFG simplification
  → control-flow structure identification
  → Java-like source rendering
```

The CLI implementation and reusable file/APK interfaces live in `src/cli.rs`. `src/main.rs` is
only a thin executable shell around `radguard::cli::run()`.

## Benchmark

### TencentQQ APK

The following measurements were supplied from WSL Ubuntu while exporting every class to individual
Java files. The APK contains 37 DEX files and RadGuard exported 325,185 classes. Times are wall-clock
results for complete APK decompilation and filesystem output.

| Workers | Wall time | Speedup vs. 8 workers | Notes |
|---:|---:|---:|---|
| 8 | 7.415386 s | 1.00× | 725% CPU, 827,840 KiB max RSS |
| 16 | 4.17 s | 1.78× | Reported run |
| 32 | 2.562731 s | 2.89× | 2166% CPU, 2,639,588 KiB max RSS |

Benchmark command:

```bash
/usr/bin/time --verbose \
  radguard-linux-source/target/release/radguard \
  export-all-parallel TencentQQ.apk 32 dumpqq
```

Benchmark results depend on CPU topology, available memory, storage, filesystem, kernel caches,
APK contents, and compiler/toolchain versions. These numbers should not be treated as universal
performance guarantees.

<details>
<summary>Full 8-worker benchmark output</summary>

```text
wsl@ubuntu:~/radguard$ /usr/bin/time --verbose radguard-linux-source/target/release/radguard export-all-parallel TencentQQ.apk 8 dumpqq
APK export: 37 DEX files, 8 parallel workers
[apk 1/37   2.7%] classes.dex complete in 0.007s
[apk 2/37   5.4%] classes3.dex complete in 1.169s
[apk 3/37   8.1%] classes8.dex complete in 1.301s
[apk 4/37  10.8%] classes2.dex complete in 1.323s
[apk 5/37  13.5%] classes9.dex complete in 1.373s
[apk 6/37  16.2%] classes7.dex complete in 1.514s
[apk 7/37  18.9%] classes4.dex complete in 1.733s
[apk 8/37  21.6%] classes5.dex complete in 1.739s
[apk 9/37  24.3%] classes6.dex complete in 1.788s
[apk 10/37  27.0%] classes10.dex complete in 1.814s
[apk 11/37  29.7%] classes13.dex complete in 1.711s
[apk 12/37  32.4%] classes12.dex complete in 1.844s
[apk 13/37  35.1%] classes17.dex complete in 1.432s
[apk 14/37  37.8%] classes11.dex complete in 1.918s
[apk 15/37  40.5%] classes14.dex complete in 1.930s
[apk 16/37  43.2%] classes16.dex complete in 1.736s
[apk 17/37  45.9%] classes15.dex complete in 1.806s
[apk 18/37  48.6%] classes21.dex complete in 1.360s
[apk 19/37  51.4%] classes24.dex complete in 1.110s
[apk 20/37  54.1%] classes22.dex complete in 1.383s
[apk 21/37  56.8%] classes25.dex complete in 1.370s
[apk 22/37  59.5%] classes18.dex complete in 1.957s
[apk 23/37  62.2%] classes20.dex complete in 1.800s
[apk 24/37  64.9%] classes19.dex complete in 1.894s
[apk 25/37  67.6%] classes23.dex complete in 1.609s
[apk 26/37  70.3%] classes29.dex complete in 0.658s
[apk 27/37  73.0%] classes28.dex complete in 1.259s
[apk 28/37  75.7%] classes30.dex complete in 1.200s
[apk 29/37  78.4%] classes26.dex complete in 1.566s
[apk 30/37  81.1%] classes32.dex complete in 1.202s
[apk 31/37  83.8%] classes33.dex complete in 1.268s
[apk 32/37  86.5%] classes27.dex complete in 1.762s
[apk 33/37  89.2%] classes31.dex complete in 1.398s
[apk 34/37  91.9%] classes36.dex complete in 0.759s
[apk 35/37  94.6%] classes34.dex complete in 1.583s
[apk 36/37  97.3%] classes35.dex complete in 1.306s
[apk 37/37 100.0%] classes37.dex complete in 1.264s
exported 325185 classes from 37 DEX file(s) to dumpqq in 7.415386s
        Command being timed: "radguard-linux-source/target/release/radguard export-all-parallel TencentQQ.apk 8 dumpqq"
        User time (seconds): 49.69
        System time (seconds): 4.10
        Percent of CPU this job got: 725%
        Elapsed (wall clock) time (h:mm:ss or m:ss): 0:07.42
        Average shared text size (kbytes): 0
        Average unshared data size (kbytes): 0
        Average stack size (kbytes): 0
        Average total size (kbytes): 0
        Maximum resident set size (kbytes): 827840
        Average resident set size (kbytes): 0
        Major (requiring I/O) page faults: 0
        Minor (reclaiming a frame) page faults: 43859
        Voluntary context switches: 157
        Involuntary context switches: 529
        Swaps: 0
        File system inputs: 0
        File system outputs: 3813512
        Socket messages sent: 0
        Socket messages received: 0
        Signals delivered: 0
        Page size (bytes): 4096
        Exit status: 0
```

</details>

<details>
<summary>Full 32-worker benchmark output</summary>

```text
wsl@ubuntu:~/radguard$ /usr/bin/time --verbose radguard-linux-source/target/release/radguard export-all-parallel TencentQQ.apk 32 dumpqq
APK export: 37 DEX files, 32 parallel workers
[apk 1/37   2.7%] classes.dex complete in 0.015s
[apk 2/37   5.4%] classes29.dex complete in 0.745s
[apk 3/37   8.1%] classes24.dex complete in 1.130s
[apk 4/37  10.8%] classes3.dex complete in 1.219s
[apk 5/37  13.5%] classes30.dex complete in 1.232s
[apk 6/37  16.2%] classes32.dex complete in 1.235s
[apk 7/37  18.9%] classes33.dex complete in 1.318s
[apk 8/37  21.6%] classes28.dex complete in 1.325s
[apk 9/37  24.3%] classes8.dex complete in 1.364s
[apk 10/37  27.0%] classes2.dex complete in 1.415s
[apk 11/37  29.7%] classes25.dex complete in 1.423s
[apk 12/37  32.4%] classes9.dex complete in 1.460s
[apk 13/37  35.1%] classes22.dex complete in 1.445s
[apk 14/37  37.8%] classes17.dex complete in 1.463s
[apk 15/37  40.5%] classes31.dex complete in 1.463s
[apk 16/37  43.2%] classes7.dex complete in 1.601s
[apk 17/37  45.9%] classes26.dex complete in 1.596s
[apk 18/37  48.6%] classes21.dex complete in 1.616s
[apk 19/37  51.4%] classes23.dex complete in 1.648s
[apk 20/37  54.1%] classes13.dex complete in 1.773s
[apk 21/37  56.8%] classes5.dex complete in 1.807s
[apk 22/37  59.5%] classes27.dex complete in 1.787s
[apk 23/37  62.2%] classes16.dex complete in 1.815s
[apk 24/37  64.9%] classes12.dex complete in 1.830s
[apk 25/37  67.6%] classes4.dex complete in 1.853s
[apk 26/37  70.3%] classes6.dex complete in 1.868s
[apk 27/37  73.0%] classes10.dex complete in 1.885s
[apk 28/37  75.7%] classes15.dex complete in 1.881s
[apk 29/37  78.4%] classes11.dex complete in 1.949s
[apk 30/37  81.1%] classes19.dex complete in 1.949s
[apk 31/37  83.8%] classes14.dex complete in 1.981s
[apk 32/37  86.5%] classes36.dex complete in 0.786s
[apk 33/37  89.2%] classes18.dex complete in 2.001s
[apk 34/37  91.9%] classes20.dex complete in 2.019s
[apk 35/37  94.6%] classes34.dex complete in 1.601s
[apk 36/37  97.3%] classes35.dex complete in 1.321s
[apk 37/37 100.0%] classes37.dex complete in 1.296s
exported 325185 classes from 37 DEX file(s) to dumpqq in 2.562731s
        Command being timed: "radguard-linux-source/target/release/radguard export-all-parallel TencentQQ.apk 32 dumpqq"
        User time (seconds): 50.33
        System time (seconds): 5.31
        Percent of CPU this job got: 2166%
        Elapsed (wall clock) time (h:mm:ss or m:ss): 0:02.56
        Average shared text size (kbytes): 0
        Average unshared data size (kbytes): 0
        Average stack size (kbytes): 0
        Average total size (kbytes): 0
        Maximum resident set size (kbytes): 2639588
        Average resident set size (kbytes): 0
        Major (requiring I/O) page faults: 1
        Minor (reclaiming a frame) page faults: 10296
        Voluntary context switches: 808
        Involuntary context switches: 532
        Swaps: 0
        File system inputs: 0
        File system outputs: 3813512
        Socket messages sent: 0
        Socket messages received: 0
        Signals delivered: 0
        Page size (bytes): 4096
        Exit status: 0
```

</details>

## Prebuilt releases

Pushing a version tag automatically runs the complete quality gate, cross-compiles RadGuard, and
publishes a GitHub Release. The tag must match the version in `Cargo.toml`.

For example, with `version = "0.1.0"`:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The release workflow publishes SHA-256-checked archives for:

| Operating system | Architecture | Rust target |
|---|---|---|
| Linux | x86-64 | `x86_64-unknown-linux-gnu` |
| Linux | ARM64/AArch64 | `aarch64-unknown-linux-gnu` |
| Linux | ARMv7 32-bit | `armv7-unknown-linux-gnueabihf` |
| Windows | x86-64 | `x86_64-pc-windows-msvc` |
| Windows | x86 32-bit | `i686-pc-windows-msvc` |
| Windows | ARM64 | `aarch64-pc-windows-msvc` |
| macOS | Intel x86-64 | `x86_64-apple-darwin` |
| macOS | Apple Silicon | `aarch64-apple-darwin` |

GitHub also generates source `.zip` and `.tar.gz` archives automatically. Release publication is
blocked unless the Rust tests, Clippy, release build, Phase-1 zero-diff gate, and Phase-2 `<=70`
gate all pass.

The workflow is defined in `.github/workflows/release.yml`.

## Tests

Run every local gate with one command:

```bash
bash scripts/check-all.sh
```

On Windows PowerShell:

```powershell
./scripts/check-all.ps1
```

The same script is used by `.github/workflows/quality-gate.yml` for pushes and pull requests.

Rust unit and integration tests:

```bash
cargo test --lib --tests --locked
cargo clippy --all-targets --locked -- -D warnings
```

Offline DAD quality gates:

```bash
python3 tests/quality_gate.py --phase 1
python3 tests/quality_gate.py --phase 2
```

The quality gate reads the checked-in `tests/fixtures/dad_reference_cache/dad-*.pickle.gz` files. They
contain frozen Androguard DAD source plus the frozen DEX-aware comparison oracles. The script does
not generate DAD output and does not import or run Androguard. Phase 1 requires exactly zero
mismatching classes. Phase 2 first runs Phase 1, then requires at most 70 mismatching classes.

The benchmark transcripts above document the current large-APK throughput results. The included
Rust test suite covers parsing, lowering, data flow, control flow, public APIs, and parallel output.
