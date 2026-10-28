import struct
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
            [], [], [], [], [], [], [], {}, self.declarations,
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


if __name__ == "__main__":
    unittest.main()
