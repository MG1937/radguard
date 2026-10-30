use radguard::Decompiler;

const DEX: &[u8] = include_bytes!("fixtures/exception_handlers.dex");

fn decompile(class: &str) -> String {
    Decompiler::parse(DEX)
        .unwrap()
        .decompile_class(class)
        .unwrap()
        .unwrap()
        .source
}

fn method<'a>(source: &'a str, name: &str) -> &'a str {
    let signature = format!(" {name}()");
    let start = source.find(&signature).expect("fixture method exists");
    let rest = &source[start..];
    let end = rest.find("\n    }\n").expect("method closing brace") + 7;
    &rest[..end]
}

fn catch_types(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter_map(|line| line.split_once("catch (").map(|(_, tail)| tail))
        .map(|tail| {
            let header = tail.split(')').next().unwrap();
            header.rsplit_once(' ').map_or(header, |(ty, _)| ty)
        })
        .collect()
}

// Inspect the scopes of fixture calls rather than matching a complete source
// snapshot. In particular, each call must appear once and outside() must not
// inherit a preceding catch region.
fn calls_and_try_depth(source: &str) -> Vec<(&str, usize)> {
    let mut scopes = Vec::new();
    let mut depth = 0;
    let mut calls = Vec::new();
    for line in source.lines().map(str::trim) {
        if line.starts_with('}') && scopes.pop().expect("balanced fixture source") {
            depth -= 1;
        }
        if let Some((_, call)) = line.split_once("exceptions.Support.") {
            calls.push((call.split('(').next().unwrap(), depth));
        }
        if line.ends_with('{') {
            let is_try = line == "try {";
            scopes.push(is_try);
            depth += usize::from(is_try);
        }
    }
    assert!(scopes.is_empty(), "unbalanced fixture source: {source}");
    calls
}

#[test]
fn catch_types_survive_exception_register_reuse() {
    let source = decompile("Lexceptions/CatchTypes;");
    assert_eq!(
        catch_types(method(&source, "reuseForInt")),
        ["IllegalArgumentException", "IllegalStateException"],
    );
    assert_eq!(
        catch_types(method(&source, "reuseForReference")),
        ["IllegalArgumentException"],
    );
}

#[test]
fn catch_all_and_multi_catch_keep_handler_types_and_order() {
    let source = decompile("Lexceptions/CatchTypes;");
    assert_eq!(catch_types(method(&source, "catchAllOnly")), ["Throwable"]);
    assert_eq!(
        catch_types(method(&source, "typedAndCatchAll")),
        ["IllegalArgumentException", "Throwable"],
    );
    assert_eq!(
        catch_types(method(&source, "multiCatch")),
        ["IllegalArgumentException | IllegalStateException"],
    );
}

#[test]
fn adjacent_tries_remain_sequential_and_do_not_duplicate_calls() {
    let source = decompile("Lexceptions/AdjacentTry;");
    for name in ["two", "sameType"] {
        assert_eq!(
            calls_and_try_depth(method(&source, name)),
            [
                ("first", 1),
                ("caughtFirst", 0),
                ("second", 1),
                ("caughtSecond", 0),
                ("outside", 0),
            ],
        );
    }
    assert_eq!(
        calls_and_try_depth(method(&source, "three")),
        [
            ("first", 1),
            ("caughtFirst", 0),
            ("second", 1),
            ("caughtSecond", 0),
            ("third", 1),
            ("caughtThird", 0),
            ("outside", 0),
        ],
    );
}

#[test]
fn nested_handlers_are_not_flattened_as_adjacent_tries() {
    let source = decompile("Lexceptions/AdjacentTry;");
    assert_eq!(
        calls_and_try_depth(method(&source, "nested")),
        [
            ("first", 1),
            ("second", 2),
            ("outerTail", 1),
            ("caughtFirst", 0),
        ],
    );
}

#[test]
fn corrected_handlers_keep_member_spans_and_annotation_modes() {
    for class in ["Lexceptions/CatchTypes;", "Lexceptions/AdjacentTry;"] {
        let output = Decompiler::parse(DEX)
            .unwrap()
            .decompile_class(class)
            .unwrap()
            .unwrap();
        let plain = Decompiler::parse(DEX)
            .unwrap()
            .without_metadata()
            .decompile_class(class)
            .unwrap()
            .unwrap();
        assert_eq!(output, plain);
        for reference in &output.member_references {
            assert_eq!(&output.source[reference.start..reference.end], reference.name);
        }
    }
}
