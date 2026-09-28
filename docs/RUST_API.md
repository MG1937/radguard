# Embedding RadGuard in a Rust project

RadGuard exposes its reusable DEX/APK operations through `radguard::cli`. The binary in
`src/main.rs` is intentionally only a thin command-line shell.

## Add the dependency

When RadGuard is checked out next to your project:

```toml
[dependencies]
radguard = { path = "../radguard" }
```

The crate uses Rust edition 2024. A recent stable Rust toolchain and a C compiler are required;
`libdeflater` builds the bundled libdeflate implementation.

## Decompile one class

The normal API includes annotations and member-reference metadata:

```rust
use radguard::cli::decompile_class_file;

fn main() -> radguard::Result<()> {
    if let Some(output) = decompile_class_file(
        "classes.dex",
        "Lcom/example/MainActivity;",
    )? {
        println!("{}", output.source);
        println!("{} member references", output.member_references.len());
    }
    Ok(())
}
```

For annotation-free historical DAD rendering:

```rust
let source = radguard::cli::decompile_class_file_dad(
    "classes.dex",
    "Lcom/example/MainActivity;",
)?;
```

For data already in memory, use `radguard::Decompiler` directly:

```rust
let decompiler = radguard::Decompiler::parse(&dex_bytes)?;
let output = decompiler.decompile_class_with_metadata("Lcom/example/MainActivity;")?;
```

## Export an APK or DEX as Java files

`ExportOptions::default()` includes annotations and runs sequentially:

```rust
use radguard::cli::{ExportOptions, export_to_directory};

let summary = export_to_directory(
    "app.apk",
    "output",
    ExportOptions::default(),
)?;
println!(
    "exported {} classes from {} DEX files in {:?}",
    summary.classes,
    summary.dex_files,
    summary.duration,
);
```

Use all available logical CPUs:

```rust
let summary = export_to_directory(
    "app.apk",
    "output",
    ExportOptions::parallel(),
)?;
```

Use an explicit global worker limit:

```rust
let options = ExportOptions::default().with_threads(8);
let summary = export_to_directory("app.apk", "output", options)?;
```

For APKs with multiple DEX files, the worker budget is used across DEX files. Different
`classesN.dex` entries are decompressed with libdeflate and decompiled concurrently. A one-DEX
APK uses the worker budget for class-level parallelism.

## Progress callback

The callback can run concurrently on worker threads, so it must not rely on thread-local ordering:

```rust
use radguard::cli::{ExportOptions, ExportProgress, export_to_directory};

fn progress(event: ExportProgress) {
    match event {
        ExportProgress::Started { total_dex_files, worker_count } => {
            eprintln!("starting {total_dex_files} DEX files on {worker_count} workers");
        }
        ExportProgress::DexCompleted {
            completed_dex_files,
            total_dex_files,
            dex_number,
            dex_duration,
        } => {
            eprintln!(
                "{completed_dex_files}/{total_dex_files}: classes{dex_number}.dex in {dex_duration:?}",
            );
        }
    }
}

let options = ExportOptions::parallel().with_progress(progress);
export_to_directory("app.apk", "output", options)?;
```

## Export to an in-memory or custom writer

`export_to_writer` writes the stable `RGOUT001` aggregate stream. This avoids creating one file
per class and accepts any `std::io::Write` implementation:

```rust
use radguard::cli::{ExportOptions, export_to_writer};

let mut archive = Vec::new();
let summary = export_to_writer(
    "app.apk",
    &mut archive,
    ExportOptions::parallel(),
)?;
```

The stream layout is:

```text
8 bytes   magic: RGOUT001
f64 LE    elapsed seconds
u32 LE    class count
repeated class count times:
  u32 LE  descriptor byte length
  u64 LE  source byte length
  bytes   UTF-8 descriptor
  bytes   UTF-8 Java-like source
```

## Annotation-free DAD compatibility

Annotations are enabled by default. Disable them only when historical DAD-compatible output is
required:

```rust
let options = ExportOptions::parallel().dad_compatible();
radguard::cli::export_to_directory("app.apk", "dad-output", options)?;
```

## Inspect a DEX

```rust
let summary = radguard::cli::inspect_file("classes.dex")?;
println!("{} classes, parsed in {:?}", summary.classes, summary.parse_duration);
```

## CLI shell

The executable calls the same APIs:

```bash
radguard export-all app.apk output
radguard export-all-parallel app.apk output
radguard export-all-parallel app.apk 8 output
```

Normal `decompile`, `export-all`, and `export-all-parallel` output includes annotations.
`decompile-dad` remains available for annotation-free single-class compatibility output.
