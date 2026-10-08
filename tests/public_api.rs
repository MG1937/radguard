use std::fs;
use std::process::Command;

use radguard::cli::{ExportOptions, decompile_class_file, export_to_directory, export_to_writer};

#[test]
fn public_file_api_exports_annotated_sources_and_stream() {
    let class = decompile_class_file(
        "tests/fixtures/sample.dex",
        "Lkotlinx/coroutines/flow/AbstractFlow$collect$1;",
    )
    .expect("decompile class")
    .expect("known class");
    assert!(class.source.contains("class AbstractFlow$collect$1"));

    let mut stream = Vec::new();
    let stream_summary = export_to_writer(
        "tests/fixtures/sample.dex",
        &mut stream,
        ExportOptions::default().with_threads(2),
    )
    .expect("export stream");
    assert_eq!(&stream[..8], b"RGOUT001");
    assert_eq!(stream_summary.dex_files, 1);
    assert_eq!(stream_summary.classes, 536);

    let output = std::env::temp_dir().join(format!(
        "radguard-public-api-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_dir_all(&output);
    let directory_summary = export_to_directory(
        "tests/fixtures/sample.dex",
        &output,
        ExportOptions::default().with_threads(2),
    )
    .expect("export directory");
    assert_eq!(directory_summary.classes, 536);
    assert!(
        output
            .join("kotlinx/coroutines/flow/AbstractFlow$collect$1.java")
            .is_file()
    );
    fs::remove_dir_all(output).expect("remove temporary export");
}

#[test]
fn cli_uses_one_command_with_metadata_enabled_by_default() {
    let binary = env!("CARGO_BIN_EXE_radguard");
    let descriptor = "Lkotlinx/coroutines/internal/ArrayQueue;";
    let annotated = Command::new(binary)
        .args(["decompile", "tests/fixtures/sample.dex", descriptor])
        .output()
        .expect("run annotated decompile");
    assert!(annotated.status.success());
    assert!(String::from_utf8_lossy(&annotated.stdout).contains("@Signature"));

    let plain = Command::new(binary)
        .args([
            "decompile",
            "--no-metadata",
            "tests/fixtures/sample.dex",
            descriptor,
        ])
        .output()
        .expect("run decompile without metadata");
    assert!(plain.status.success());
    assert!(!String::from_utf8_lossy(&plain.stdout).contains("@Signature"));

    let removed_command = Command::new(binary)
        .args(["decompile-dad", "tests/fixtures/sample.dex", descriptor])
        .output()
        .expect("run removed command");
    assert_eq!(removed_command.status.code(), Some(2));
}
