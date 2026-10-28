# Method parameter regression fixture

These hand-written Smali classes reproduce missing parameters in method
declarations without code. `../method_parameters.dex` is assembled from these
sources; it contains no code copied from an application.

- `ParameterInterface`: a single `long` parameter, mixed narrow/wide/reference/
  array parameters, zero parameters, and three overloads of the same method.
- `AbstractParameters`: abstract mixed-parameter and zero-parameter methods.
- `MethodParameters`: instance and static native methods, a constructor, and
  concrete instance/static methods with local registers and wide parameters.
  The concrete methods return a parameter so tests also check that declaration
  names still agree with names in the body.

Without code, names are positional (`p0`, `p1`, ...), independent of register
width and the implicit receiver. With code, existing register-based names are
preserved.

To regenerate from the repository root using Google Smali 3.0.10 and its runtime
dependencies on `SMALI_CLASSPATH`:

```sh
java -cp "$SMALI_CLASSPATH" com.android.tools.smali.smali.Main assemble \
  --api 21 --jobs 1 tests/fixtures/method_parameters \
  -o tests/fixtures/method_parameters.dex
cargo test --locked --test method_parameters
python3 -S -m unittest discover -s tests -p test_quality_gate.py
```

The checked-in DEX is 1,308 bytes, with SHA-256
`29161491f27e309cc8be8bfc8fc0e51f5c43f774a4ab15a86b8f6db0a5ec28f2`.
Running the Rust tests does not require Java or Smali.

The frozen DAD reference has the same missing-parameter bug. The quality gate
reads `code_off` and method prototypes directly from the original DEX and fills
only empty parameter lists in DAD declarations, preserving class-data order for
overloads. Candidate output, mismatch thresholds, whitelists, and frozen cache
files are unchanged. A missing or mismatched reference declaration fails the
check. Comparator tests verify that missing, extra, reordered, or mistyped
candidate parameters still fail, as do unrelated method-body changes.

To verify this negative control against a pre-fix release binary:

```sh
python3 -S tests/quality_gate.py --phase 1 --no-build --rust-bin /path/to/old/radguard
```

That command must fail with 26 mismatching classes in `sample.dex`.
