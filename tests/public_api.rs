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

#[test]
fn decompiler_recovers_jadx_aligned_literals_comparisons_and_constructors() {
    let bytes = fs::read("tests/fixtures/classes5.dex").expect("read classes5.dex");
    let decompiler = radguard::Decompiler::parse(&bytes).expect("parse classes5.dex");
    let ratingbar = decompiler
        .decompile_class("Lcom/maoyan/android/common/view/ratingbar/b;")
        .expect("decompile ratingbar")
        .expect("ratingbar exists");

    assert!(
        ratingbar.source.contains("super.f = -1.0f;"),
        "expected float literal -1.0f in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains("this(p4, null);")
            && ratingbar.source.contains("android.graphics.drawable.Drawable v10 = null;")
            && ratingbar.source.contains(
                "android.graphics.drawable.Drawable v7_15 = (!v4_5.hasValue(2)) ? null : androidx.core.content.ContextCompat.getDrawable(p12, v4_5.getResourceId(2, -1));"
            ),
        "expected null literal and ternary expression recovery in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains("int[] v4_4 = new int[] {2130971689, 2130971690, 2130971691"),
        "expected folded array initializer in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains("for (com.maoyan.android.common.view.ratingbar.c v1_5 : this.r)")
            && ratingbar.source.contains("for (int v0_4 = 1; v0_4 <= this.a; v0_4++)"),
        "expected structured for-each and counted for loops in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains("com.meituan.android.paladin.Paladin.record(1059516409037005436L);"),
        "expected L suffix on 64-bit long constant in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains("super.g = 0.1f;"),
        "expected float literal 0.1f in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains("if (((double) v2_4) <= vtmp8)")
            && ratingbar.source.contains("if (((double) v2_4) != vtmp8)"),
        "expected multi-use cmpl-double condition folding without raw cmp in:\n{}",
        ratingbar.source
    );
    assert!(
        !ratingbar.source.contains(" cmp "),
        "unexpected raw cmp operator in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains(
            "com.maoyan.android.common.view.ratingbar.c v9 = new com.maoyan.android.common.view.ratingbar.c(this.getContext(), v0_4, this.c, this.d, this.b);"
        ),
        "expected folded constructor call in:\n{}",
        ratingbar.source
    );
    assert!(
        ratingbar.source.contains(
            "com.maoyan.android.common.view.ratingbar.SavedState v1_2 = new com.maoyan.android.common.view.ratingbar.SavedState(super.onSaveInstanceState());"
        ),
        "expected inferred constructor type instead of void in:\n{}",
        ratingbar.source
    );

    let build_config = decompiler
        .decompile_class("Lcom/meituan/ai/speech/fusetts/BuildConfig;")
        .expect("decompile BuildConfig")
        .expect("BuildConfig exists");
    assert!(
        build_config.source.contains("public static final boolean DEBUG = false;"),
        "expected Java boolean literal false in:\n{}",
        build_config.source
    );
    assert!(
        build_config.source.contains("public static final Integer SDK_VERSION_CODE = null;"),
        "expected Java null literal in:\n{}",
        build_config.source
    );

    let cache_info = decompiler
        .decompile_class("Lcom/meituan/ai/speech/tts/base/data/cache/CacheInfoManager;")
        .expect("decompile CacheInfoManager")
        .expect("CacheInfoManager exists");
    assert!(
        cache_info.source.contains("public static final long BASE_DISK_LRU_CACHE_SIZE = 1048576L;"),
        "expected Java long literal with L suffix in:\n{}",
        cache_info.source
    );

    let hlist = decompiler
        .decompile_class("Lcom/meituan/android/base/ui/widget/HorizontalListView;")
        .expect("decompile HorizontalListView")
        .expect("HorizontalListView exists");
    assert!(
        hlist.source.contains("public static final float FLING_DEFAULT_ABSORB_VELOCITY = 30.0f;")
            && hlist.source.contains("public static final float FLING_FRICTION = 0.009f;"),
        "expected Java float field initializers with f suffix in:\n{}",
        hlist.source
    );

    let scene = decompiler
        .decompile_class("Lcom/meituan/android/addresscenter/scene/a;")
        .expect("decompile scene/a")
        .expect("scene/a exists");
    assert!(
        scene.source.contains(">>> 59"),
        "expected unsigned shift >>> in:\n{}",
        scene.source
    );

    let sample_bytes = fs::read("tests/fixtures/sample.dex").expect("read sample.dex");
    let sample_dec = radguard::Decompiler::parse(&sample_bytes).expect("parse sample.dex");
    let array_queue = sample_dec
        .decompile_class("Lkotlinx/coroutines/internal/ArrayQueue;")
        .expect("decompile ArrayQueue")
        .expect("ArrayQueue exists");
    assert!(
        array_queue.source.contains("public class ArrayQueue<T> {")
            && array_queue.source.contains("public final void addLast(T p3_element)")
            && array_queue.source.contains("boolean v0_1 = this.head == this.tail;"),
        "expected generic class signature, debug parameter name p3_element, and simplified boolean equality in:\n{}",
        array_queue.source
    );
}
