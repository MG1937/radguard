import hashlib
import json
import struct
import tempfile
import unittest
from pathlib import Path

import quality_gate as gate


FIXTURES = Path(__file__).resolve().parent / "fixtures"
INTERFACE = "Lparameters/ParameterInterface;"
FROZEN = """public interface ParameterInterface {
    public abstract double applyAsDouble();
    public abstract void mixed();
    public abstract void noParameters();
    public abstract void overloaded();
    public abstract void overloaded();
    public abstract void overloaded();
}
"""
CORRECT = """public interface ParameterInterface {
    public abstract double applyAsDouble(long p0);
    public abstract void mixed(int p0, long p1, double p2, String p3, int[] p4);
    public abstract void noParameters();
    public abstract void overloaded();
    public abstract void overloaded(int p0);
    public abstract void overloaded(long p0, double p1);
}
"""


class BodylessParameterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dex = (FIXTURES / "method_parameters.dex").read_bytes()
        cls.oracle = gate.bodyless_parameter_oracle(cls.dex)
        cls.declarations = cls.oracle[INTERFACE]

    def compare(self, candidate, reference=FROZEN):
        return gate.compare_class((
            INTERFACE, reference, candidate,
            [], [], [], [], [], [], [], {}, self.declarations, [], {},
        ))

    def test_original_dex_is_the_signature_authority(self):
        self.assertEqual(self.declarations, [
            ("D", "applyAsDouble", ["J"]),
            ("V", "mixed", ["I", "J", "D", "Ljava/lang/String;", "[I"]),
            ("V", "noParameters", []),
            ("V", "overloaded", []),
            ("V", "overloaded", ["I"]),
            ("V", "overloaded", ["J", "D"]),
        ])
        self.assertEqual(sum(map(len, self.oracle.values())), 12)
        native = self.oracle["Lparameters/MethodParameters;"]
        self.assertEqual([name for _, name, _ in native], [
            "staticNativeMixed", "staticNativeNoParameters",
            "nativeMixed", "nativeNoParameters",
        ])
        self.assertEqual(native[0][2], ["I", "J", "D", "Ljava/lang/String;", "[I"])
        self.assertEqual(native[2][2], native[0][2])
        self.assertNotIn("<init>", [name for _, name, _ in native])

    def test_only_missing_reference_parameters_are_restored(self):
        self.assertEqual(gate.restore_dad_bodyless_parameters(FROZEN, self.declarations), CORRECT)
        self.assertEqual(gate.restore_dad_bodyless_parameters(CORRECT, self.declarations), CORRECT)
        wrong_reference = CORRECT.replace("long p0", "float p0", 1)
        self.assertEqual(
            gate.restore_dad_bodyless_parameters(wrong_reference, self.declarations),
            wrong_reference,
        )

    def test_fixed_candidate_passes_but_original_missing_parameters_fail(self):
        self.assertIsNone(self.compare(CORRECT))
        self.assertIsNotNone(self.compare(FROZEN))

    def test_wrong_count_type_order_and_overloads_remain_failures(self):
        for candidate in [
            CORRECT.replace("long p0", "int p0", 1),
            CORRECT.replace("long p1, double p2", "double p1, long p2"),
            CORRECT.replace("String p3, int[] p4", "String p3"),
            CORRECT.replace("int[] p4", "String[] p4"),
            CORRECT.replace("noParameters()", "noParameters(int p0)"),
            CORRECT.replace("overloaded(int p0)", "overloaded()"),
            CORRECT.replace("overloaded(long p0, double p1)", "overloaded(double p0, long p1)"),
            CORRECT.replace("    public abstract void noParameters();\n", ""),
        ]:
            with self.subTest(candidate=candidate):
                self.assertIsNotNone(self.compare(candidate))

    def test_body_changes_and_other_declarations_remain_visible(self):
        suffix = """    public int body()
    {
        return 0;
    }
    Object field = factory();
"""
        reference = FROZEN + suffix
        candidate = CORRECT + suffix
        self.assertEqual(
            gate.restore_dad_bodyless_parameters(reference, self.declarations),
            candidate,
        )
        self.assertIsNone(self.compare(candidate, reference))
        self.assertIsNotNone(self.compare(candidate.replace("return 0;", "return 1;"), reference))
        self.assertIsNotNone(self.compare(candidate.replace("factory()", "other()"), reference))

    def test_reference_order_and_missing_declarations_fail_closed(self):
        for reference in [
            FROZEN.replace("    public abstract void mixed();\n", ""),
            FROZEN.replace("applyAsDouble", "differentMethod"),
            FROZEN + "    public native void extra();\n",
        ]:
            with self.subTest(reference=reference):
                with self.assertRaises(ValueError):
                    gate.restore_dad_bodyless_parameters(reference, self.declarations)

    def test_empty_oracle_does_not_hide_an_unexpected_reference_declaration(self):
        with self.assertRaises(ValueError):
            gate.restore_dad_bodyless_parameters(FROZEN, [])
        self.assertEqual(gate.restore_dad_bodyless_parameters("class Empty {}\n", []), "class Empty {}\n")

    def test_existing_dex_fixtures_include_mutf8_and_native_declarations(self):
        for name, expected_count in [("sample.dex", 94), ("classes5.dex", 2917)]:
            with self.subTest(name=name):
                oracle = gate.bodyless_parameter_oracle((FIXTURES / name).read_bytes())
                self.assertEqual(sum(map(len, oracle.values())), expected_count)

    def test_incompatible_dex_headers_are_rejected(self):
        for data in [b"", self.dex[:80], self.dex[:4] + b"041\0" + self.dex[8:]]:
            with self.subTest(header=data[:8]):
                with self.assertRaises(ValueError):
                    gate.bodyless_parameter_oracle(data)
        wrong_endian = bytearray(self.dex)
        struct.pack_into("<I", wrong_endian, 40, 0x78563412)
        with self.assertRaises(ValueError):
            gate.bodyless_parameter_oracle(bytes(wrong_endian))


class CatchTypeTests(unittest.TestCase):
    SOURCE = """public class C {
    public void run()
    {
        try {
            first();
        } catch (boolean v0) {
            caughtFirst();
        }
        try {
            second();
        } catch (String v1) {
            caughtSecond();
        }
    }
}
"""

    @classmethod
    def setUpClass(cls):
        cls.correct = cls.SOURCE.replace("boolean v0", "Throwable v0").replace(
            "String v1", "Throwable v1"
        )

    def compare(self, candidate):
        return gate.compare_class((
            "LC;", self.SOURCE, candidate,
            [], [], [], [], [], [], [[['Throwable']]], {}, [], [[(0, ["Throwable"])]], {},
        ))

    def test_types_and_exception_registers_come_from_original_dex_handlers(self):
        bodyless, catches = gate.method_metadata_oracles(
            (FIXTURES / "exception_handlers.dex").read_bytes()
        )
        self.assertEqual(bodyless, {})
        self.assertEqual(catches["Lexceptions/CatchTypes;"], [
            [(0, ["Throwable"])],
            [(0, ["IllegalArgumentException", "IllegalStateException"])],
            [(0, ["IllegalArgumentException"]), (0, ["IllegalStateException"])],
            [(0, ["IllegalArgumentException"])],
            [(0, ["IllegalArgumentException"]), (0, ["Throwable"])],
        ])
        self.assertEqual(catches["Lexceptions/AdjacentTry;"], [
            [(0, ["IllegalStateException"]), (0, ["IllegalArgumentException"])],
            [(0, ["IllegalArgumentException"]), (0, ["IllegalArgumentException"])],
            [(0, ["IllegalArgumentException"]), (0, ["IllegalStateException"]), (0, ["UnsupportedOperationException"])],
            [(0, ["IllegalArgumentException"]), (0, ["IllegalStateException"])],
        ])

    def test_repeated_homogeneous_catches_correct_only_the_reference(self):
        dad, candidate = gate.normalize_dad_catch_types(
            self.SOURCE, self.correct, [[['Throwable']]], [[(0, ["Throwable"])]]
        )
        self.assertEqual(dad, self.correct.rstrip('\n'))
        self.assertEqual(candidate, self.correct.rstrip('\n'))
        self.assertIsNone(self.compare(self.correct))
        self.assertIsNotNone(self.compare(self.SOURCE))

    def test_wrong_candidate_types_and_body_changes_still_fail(self):
        for candidate in [
            self.correct.replace("Throwable v0", "Exception v0"),
            self.correct.replace("Throwable v1", "int v1"),
            self.correct.replace("caughtFirst()", "different()"),
            self.correct.replace("second();", "third();"),
            self.correct.replace("        } catch (Throwable v1) {\n            caughtSecond();\n", ""),
        ]:
            with self.subTest(candidate=candidate):
                self.assertIsNotNone(self.compare(candidate))

    def test_missing_or_ambiguous_metadata_is_not_a_blanket_correction(self):
        for targets in [None, [], [[(0, ["IllegalArgumentException"]), (0, ["IllegalStateException"])]]]:
            with self.subTest(metadata=targets):
                dad, candidate = gate.normalize_dad_catch_types(
                    self.SOURCE, self.SOURCE,
                    [[['IllegalArgumentException'], ['IllegalStateException']]],
                    targets,
                )
                self.assertEqual(dad, self.SOURCE.rstrip('\n'))
                self.assertEqual(candidate, self.SOURCE.rstrip('\n'))

    def test_mixed_types_follow_unique_registers_independently_of_order(self):
        reference = self.SOURCE.replace("boolean", "ClassNotFoundException").replace("String", "java.util.List")
        correct = reference.replace("java.util.List", "Throwable")
        groups = [[['Throwable'], ['ClassNotFoundException']]]
        targets = [[(1, ['Throwable']), (0, ['ClassNotFoundException'])]]
        dad, candidate = gate.normalize_dad_catch_types(reference, correct, groups, targets)
        self.assertEqual(dad, correct.rstrip('\n'))
        self.assertEqual(candidate, correct.rstrip('\n'))
        wrong = correct.replace("Throwable", "Exception")
        dad, candidate = gate.normalize_dad_catch_types(reference, wrong, groups, targets)
        self.assertNotEqual(
            gate.semantic_source(dad, dad_source=True),
            gate.semantic_source(candidate, dad_source=False),
        )

    def test_split_register_names_keep_candidate_types_visible(self):
        reference = self.SOURCE.replace('v0', 'v0_2').replace('v1', 'v1_3')
        correct = reference.replace('boolean', 'Exception').replace('String', 'Throwable')
        groups = [[['Exception'], ['Throwable']]]
        targets = [[(0, ['Exception']), (1, ['Throwable'])]]
        for candidate in [correct, reference, correct.replace('Exception', 'Error')]:
            dad, rust = gate.normalize_dad_catch_types(reference, candidate, groups, targets)
            self.assertEqual(dad, correct.rstrip('\n'))
            self.assertEqual(rust, candidate.rstrip('\n'))

    def test_no_exception_register_does_not_guess_mixed_types(self):
        groups = [[['Exception'], ['Throwable']]]
        targets = [[(None, ['Exception']), (None, ['Throwable'])]]
        dad, candidate = gate.normalize_dad_catch_types(self.SOURCE, self.SOURCE, groups, targets)
        self.assertEqual(dad, self.SOURCE.rstrip('\n'))
        self.assertEqual(candidate, self.SOURCE.rstrip('\n'))

    def test_array_types_in_broken_reference_catches_are_corrected(self):
        reference = self.SOURCE.replace('boolean', 'Object[][]').replace('String', 'int[]')
        dad, candidate = gate.normalize_dad_catch_types(
            reference, self.correct, [[['Throwable']]], [[(0, ['Throwable'])]],
        )
        self.assertEqual(dad, self.correct.rstrip('\n'))
        self.assertEqual(candidate, self.correct.rstrip('\n'))


class FrozenCatchAnnotationTests(unittest.TestCase):
    def setUp(self):
        self.source = CatchTypeTests.SOURCE.replace('v1', 'v0_1')
        lines = self.source.splitlines()
        start, end = gate.method_sections(lines)[0]
        self.annotations = {0: {
            'source_sha256': hashlib.sha256('\n'.join(lines[start:end + 1]).encode()).hexdigest(),
            'catch_types': ['Exception', 'Throwable'],
        }}
        self.raw = [[(0, ['Exception']), (0, ['Throwable'])]]
        self.correct = self.source.replace('boolean', 'Exception').replace('String', 'Throwable')

    def compare(self, candidate, source=None):
        return gate.compare_class((
            'LC;', self.source if source is None else source, candidate,
            [], [], [], [], [], [], [[['Exception'], ['Throwable']]], {}, [],
            self.raw, self.annotations,
        ))

    def test_annotations_correct_only_reference_and_preserve_type_order_checks(self):
        self.assertIsNone(self.compare(self.correct))
        for candidate in [
            self.source,
            self.correct.replace('Exception v0', 'Throwable v0'),
            self.correct.replace('Exception v0', 'Throwable v0').replace('Throwable v0_1', 'Exception v0_1'),
            self.correct.replace('caughtFirst()', 'different()'),
        ]:
            with self.subTest(candidate=candidate):
                self.assertIsNotNone(self.compare(candidate))

    def test_source_hash_count_and_original_dex_types_are_required(self):
        with self.assertRaises(ValueError):
            self.compare(self.correct, self.source.replace('first()', 'different()'))
        for types in [['Throwable'], ['Error', 'Throwable']]:
            with self.subTest(types=types):
                self.annotations[0]['catch_types'] = types
                with self.assertRaises(ValueError):
                    self.compare(self.correct)

    def test_annotations_are_bound_to_the_exact_dex(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'annotations.json'
            path.write_text(json.dumps({'dex_sha256': hashlib.sha256(b'original').hexdigest(), 'records': []}))
            self.assertEqual(gate.load_catch_annotations(b'original', path), {})
            with self.assertRaises(ValueError):
                gate.load_catch_annotations(b'changed', path)

    def test_missing_instruction_metadata_leaves_reference_declaration_unchanged(self):
        self.annotations[0]['catch_types'] = ['Exception', None]
        restored = gate.restore_dad_catch_annotations(self.source, self.annotations, self.raw)
        self.assertEqual(restored, self.source.replace('boolean', 'Exception').rstrip('\n'))

    def test_checked_in_annotations_match_frozen_source_and_dex_handler_types(self):
        dex = (FIXTURES / 'classes5.dex').read_bytes()
        annotations = gate.load_catch_annotations(dex, FIXTURES / 'dad_catch_types/classes5.json')
        _, raw = gate.method_metadata_oracles(dex)
        frozen, _, _ = gate.decompile_dad_cached(dex, FIXTURES / 'dad_reference_cache', False, 'classes5.dex')
        self.assertTrue(annotations)
        for descriptor, methods in annotations.items():
            with self.subTest(descriptor=descriptor):
                gate.restore_dad_catch_annotations(frozen[descriptor], methods, raw[descriptor])


if __name__ == "__main__":
    unittest.main()
