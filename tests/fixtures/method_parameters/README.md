# Method parameter regression fixture

These hand-written Smali classes reproduce missing parameters in method
declarations without code. `../method_parameters.dex` is assembled from these
sources; it contains no code copied from an application.

- `ParameterInterface`: a single `long` parameter, mixed narrow/wide/reference/
  array parameters, and a zero-parameter declaration.
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
```

The checked-in DEX is 1,228 bytes, with SHA-256
`46037429a61d6c8788405ec28baaaf002c922945bb98cf6a09eda511a9cb785b`.
Running the Rust tests does not require Java or Smali.
