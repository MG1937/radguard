use radguard::analysis::MethodAnalysis;
use radguard::control_flow;
use radguard::dataflow;
use radguard::graph::Graph;
use radguard::ir::opcode::lower_method;
use radguard::{Decompiler, DexFile, Error};
use std::collections::BTreeMap;

fn sample_bytes() -> Vec<u8> {
    std::fs::read("tests/fixtures/sample.dex").expect("read phase-one sample")
}

#[test]
fn parses_phase_one_sample_index_and_class_data() {
    let bytes = sample_bytes();
    let dex = DexFile::parse(&bytes).expect("parse sample.dex");

    assert_eq!(dex.header.version, 39);
    assert_eq!(dex.strings().len(), 5_244);
    assert_eq!(dex.type_ids().len(), 784);
    assert_eq!(dex.protos().len(), 829);
    assert_eq!(dex.field_ids().len(), 1_333);
    assert_eq!(dex.method_ids().len(), 2_806);
    assert_eq!(dex.classes().len(), 536);

    let encoded_fields: usize = dex
        .classes()
        .iter()
        .map(|class| class.fields().count())
        .sum();
    let encoded_methods: usize = dex
        .classes()
        .iter()
        .map(|class| class.methods().count())
        .sum();
    assert_eq!(encoded_fields, 1_275);
    assert_eq!(encoded_methods, 2_364);
}

#[test]
fn resolves_phase_one_class_and_method_descriptors() {
    let bytes = sample_bytes();
    let dex = DexFile::parse(&bytes).expect("parse sample.dex");
    let descriptor = "Lkotlinx/coroutines/flow/AbstractFlow$collect$1;";
    let class = dex.class(descriptor).expect("find first class");

    assert_eq!(dex.class_descriptor(class).unwrap(), descriptor);
    assert_eq!(
        dex.class_superclass(class).unwrap(),
        Some("Lkotlin/coroutines/jvm/internal/ContinuationImpl;")
    );
    assert_eq!(class.fields().count(), 4);
    assert_eq!(class.methods().count(), 2);

    let constructor = dex
        .method(class.methods().next().unwrap().method_idx)
        .unwrap();
    assert_eq!(constructor.class, descriptor);
    assert_eq!(constructor.name, "<init>");
    assert_eq!(
        constructor.descriptor,
        "(Lkotlinx/coroutines/flow/AbstractFlow; Lkotlin/coroutines/Continuation;)V"
    );
}

#[test]
fn parses_code_headers_and_exception_handlers() {
    let bytes = sample_bytes();
    let dex = DexFile::parse(&bytes).expect("parse sample.dex");
    let mut code_methods = 0;
    let mut try_items = 0;
    for class in dex.classes() {
        for method in class.methods() {
            if let Some(code) = dex.code(method).expect("parse code item") {
                assert_eq!(code.instruction_bytes().len(), code.insns_size as usize * 2);
                code_methods += 1;
                try_items += code.tries.len();
            }
        }
    }
    assert_eq!(code_methods, 2_270);
    assert_eq!(try_items, 228);
}

#[test]
fn decodes_every_phase_one_instruction() {
    let bytes = sample_bytes();
    let dex = DexFile::parse(&bytes).expect("parse sample.dex");
    let mut counts = BTreeMap::<&str, usize>::new();
    let mut instruction_count = 0;

    for class in dex.classes() {
        for method in class.methods() {
            let Some(code) = dex.code(method).expect("parse code item") else {
                continue;
            };
            let instructions = code.decode().unwrap_or_else(|error| {
                let method = dex.method(method.method_idx).unwrap();
                panic!(
                    "decode {}->{}{} failed: {error}",
                    method.class, method.name, method.descriptor
                )
            });
            instruction_count += instructions.len();
            for instruction in &instructions {
                *counts.entry(instruction.mnemonic()).or_default() += 1;
            }
        }
    }

    assert_eq!(instruction_count, 31_503);
    assert_eq!(counts.len(), 116);
    assert_eq!(counts["move-result-object"], 2_552);
    assert_eq!(counts["const/4"], 2_413);
    assert_eq!(counts["packed-switch-payload"], 157);
}

#[test]
fn builds_phase_one_basic_blocks_and_exception_regions() {
    let bytes = sample_bytes();
    let dex = DexFile::parse(&bytes).expect("parse sample.dex");
    let mut methods = 0;
    let mut blocks = 0;
    let mut edges = 0;
    let mut exceptions = 0;
    let mut handlers = 0;

    for class in dex.classes() {
        for method in class.methods() {
            let Some(analysis) =
                MethodAnalysis::new(&dex, method).expect("analyze method control flow")
            else {
                continue;
            };
            methods += 1;
            blocks += analysis.blocks.len();
            edges += analysis
                .blocks
                .iter()
                .map(|block| block.successors.len())
                .sum::<usize>();
            exceptions += analysis.exceptions.len();
            handlers += analysis
                .exceptions
                .iter()
                .map(|exception| exception.handlers.len())
                .sum::<usize>();
        }
    }

    assert_eq!(methods, 2_270);
    assert_eq!(blocks, 7_670);
    assert_eq!(edges, 6_598);
    assert_eq!(exceptions, 228);
    assert_eq!(handlers, 233);
}

#[test]
fn lowers_every_phase_one_method_to_dad_ir_bits_ut() {
    let bytes = sample_bytes();
    let dex = DexFile::parse(&bytes).expect("parse sample.dex");
    let mut methods = 0;
    let mut blocks = 0;

    for class in dex.classes() {
        for method in class.methods() {
            let Some(lowered) = lower_method(&dex, method).unwrap_or_else(|error| {
                let method = dex.method(method.method_idx).unwrap();
                panic!(
                    "lower {}->{}{} failed: {error}",
                    method.class, method.name, method.descriptor
                )
            }) else {
                continue;
            };
            methods += 1;
            blocks += lowered.blocks.len();
        }
    }

    assert_eq!(methods, 2_270);
    assert_eq!(blocks, 7_670);
}

#[test]
fn optimizes_every_phase_one_method_graph_bits_ut() {
    let bytes = sample_bytes();
    let dex = DexFile::parse(&bytes).expect("parse sample.dex");
    let mut methods = 0;

    for class in dex.classes() {
        for method in class.methods() {
            let Some(lowered) = lower_method(&dex, method).expect("lower method") else {
                continue;
            };
            let mut graph = Graph::from_lowered(lowered);
            dataflow::optimize(&mut graph);
            graph.split_condition_nodes();
            graph.simplify();
            control_flow::identify_structures(&mut graph);
            let dominators = graph.immediate_dominators();

            assert_eq!(dominators.len(), graph.nodes.len());
            for node in graph.active_nodes() {
                for successor in graph.all_successors(node) {
                    assert!(graph.all_predecessors(successor).any(|value| value == node));
                }
            }
            methods += 1;
        }
    }

    assert_eq!(methods, 2_270);
}

#[test]
fn decompiles_every_phase_one_class_with_valid_metadata_bits_ut() {
    let bytes = sample_bytes();
    let decompiler = Decompiler::parse(&bytes).expect("parse sample.dex");
    let classes = decompiler.decompile_all().expect("decompile sample.dex");

    assert_eq!(classes.len(), 536);
    for (descriptor, output) in classes {
        assert!(
            !output.source.is_empty(),
            "{descriptor} emitted empty source"
        );
        for reference in output.member_references {
            let text = output
                .source
                .get(reference.start..reference.end)
                .unwrap_or_else(|| {
                    panic!(
                        "{descriptor} has invalid member span {}..{}",
                        reference.start, reference.end
                    )
                });
            assert!(!text.is_empty(), "{descriptor} has an empty member span");
        }
    }
}

#[test]
fn parallel_decompilation_preserves_class_order_and_metadata_bits_ut() {
    let bytes = sample_bytes();
    let decompiler = Decompiler::parse(&bytes).expect("parse sample.dex");
    let expected = decompiler
        .decompile_all_dad()
        .expect("decompile sample.dex sequentially");
    let parallel = decompiler
        .decompile_all_dad_parallel(4)
        .expect("decompile sample.dex with four workers");
    let zero_worker_fallback = decompiler
        .decompile_all_dad_parallel(0)
        .expect("decompile sample.dex with the zero-worker fallback");

    for actual in [parallel, zero_worker_fallback] {
        assert_eq!(actual.len(), expected.len());
        for ((actual_descriptor, actual_output), (expected_descriptor, _)) in
            actual.iter().zip(&expected)
        {
            assert_eq!(actual_descriptor, expected_descriptor);
            assert!(!actual_output.source.is_empty());
            for reference in &actual_output.member_references {
                let text = actual_output
                    .source
                    .get(reference.start..reference.end)
                    .unwrap_or_else(|| {
                        panic!(
                            "{actual_descriptor} has invalid member span {}..{}",
                            reference.start, reference.end
                        )
                    });
                assert!(
                    !text.is_empty(),
                    "{actual_descriptor} has an empty member span"
                );
            }
        }
    }
}

#[test]
fn rejects_truncated_and_wrong_endian_inputs() {
    assert!(matches!(
        DexFile::parse(&[0_u8; 16]),
        Err(Error::UnexpectedEof { .. })
    ));

    let mut bytes = sample_bytes();
    bytes[40..44].copy_from_slice(&0x7856_3412_u32.to_le_bytes());
    assert!(matches!(
        DexFile::parse(&bytes),
        Err(Error::UnsupportedEndian(0x7856_3412))
    ));
}
