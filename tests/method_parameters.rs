use radguard::Decompiler;
use radguard::writer::SourceOutput;

const DEX: &[u8] = include_bytes!("fixtures/method_parameters.dex");

fn decompile(class: &str) -> SourceOutput {
    Decompiler::parse(DEX)
        .expect("parse method parameter fixture")
        .decompile_class(class)
        .expect("decompile parameter fixture")
        .expect("fixture class exists")
}

fn assert_contains(source: &str, expected: &str) {
    assert!(
        source.contains(expected),
        "missing {expected:?} in:\n{source}"
    );
}

#[test]
fn interface_methods_keep_prototype_parameters_without_code() {
    let output = decompile("Lparameters/ParameterInterface;");
    assert_contains(
        &output.source,
        "public abstract double applyAsDouble(long p0);",
    );
    assert_contains(
        &output.source,
        "public abstract void mixed(int p0, long p1, double p2, String p3, int[] p4);",
    );
    assert_contains(&output.source, "public abstract void noParameters();");
    assert_contains(&output.source, "public abstract void overloaded();");
    assert_contains(&output.source, "public abstract void overloaded(int p0);");
    assert_contains(
        &output.source,
        "public abstract void overloaded(long p0, double p1);",
    );
}

#[test]
fn abstract_methods_keep_prototype_parameters_without_code() {
    let output = decompile("Lparameters/AbstractParameters;");
    assert_contains(
        &output.source,
        "public abstract void mixed(int p0, long p1, double p2, String p3, int[] p4);",
    );
    assert_contains(&output.source, "public abstract void noParameters();");
}

#[test]
fn native_methods_keep_prototype_parameters_without_code() {
    let output = decompile("Lparameters/MethodParameters;");
    assert_contains(
        &output.source,
        "public native void nativeMixed(int p0, long p1, double p2, String p3, int[] p4);",
    );
    assert_contains(
        &output.source,
        "public static native void staticNativeMixed(int p0, long p1, double p2, String p3, int[] p4);",
    );
    assert_contains(&output.source, "public native void nativeNoParameters();");
    assert_contains(
        &output.source,
        "public static native void staticNativeNoParameters();",
    );
}

#[test]
fn methods_with_code_keep_register_names_and_wide_parameter_slots() {
    let output = decompile("Lparameters/MethodParameters;");
    assert_contains(&output.source, "public MethodParameters(int p2)\n    {");
    assert_contains(
        &output.source,
        "public double instanceMixed(int p3, long p4, double p6, String p8, int[] p9)\n    {\n        return p6;\n    }",
    );
    assert_contains(
        &output.source,
        "public static String staticMixed(int p1, long p2, double p4, String p6, int[] p7)\n    {\n        return p6;\n    }",
    );
    assert_contains(&output.source, "public void concreteNoParameters()\n    {");
}

#[test]
fn parameter_rendering_preserves_member_references_and_metadata_modes() {
    for class in [
        "Lparameters/ParameterInterface;",
        "Lparameters/AbstractParameters;",
        "Lparameters/MethodParameters;",
    ] {
        let output = decompile(class);
        let plain = Decompiler::parse(DEX)
            .unwrap()
            .without_metadata()
            .decompile_class(class)
            .unwrap()
            .unwrap();
        assert_eq!(plain, output);
        for reference in output.member_references.iter().filter(|r| r.declaration) {
            let expected_name = if reference.name == "<init>" {
                "MethodParameters"
            } else {
                &reference.name
            };
            assert_eq!(
                &output.source[reference.start..reference.end],
                expected_name,
            );
        }
        if class == "Lparameters/ParameterInterface;" {
            let method = output
                .member_references
                .iter()
                .find(|reference| reference.name == "applyAsDouble")
                .unwrap();
            assert!(method.declaration);
            assert_eq!(method.descriptor, "(J)D");
        }
    }
}
