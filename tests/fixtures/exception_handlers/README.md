# Exception handler regression fixture

These original Smali sources assemble to `../exception_handlers.dex` (1,820
bytes, SHA-256
`0908b58cd8647d23d80e7f54d4ce97fc3f3408dcdd1cc8efcf17b808daf99812`).
They do not contain application code.

- `CatchTypes` reuses an exception register for integers or a string, and
  covers typed handlers, catch-all handlers, and multi-catch order.
- `AdjacentTry` has two and three sequential try regions, different and equal
  exception types, and a nested region that must remain nested. Explicit
  branches already delimit their basic blocks.

The adjacent-try fix applies to statement blocks that define no values and
have one normal successor and independent returning handlers. It does not
attempt general exception-region reconstruction or split blocks at DEX try
ends. Those cases also require work on value propagation and shared handlers.

Regenerate with Google Smali 3.0.10 and its runtime dependencies on
`SMALI_CLASSPATH`:

```sh
java -cp "$SMALI_CLASSPATH" com.android.tools.smali.smali.Main assemble \
  --api 21 --jobs 1 tests/fixtures/exception_handlers \
  -o tests/fixtures/exception_handlers.dex
cargo test --locked --test exception_handlers
```

The Rust tests require neither Java nor Smali. They inspect catch types,
nesting, call counts, normal continuations, and source-reference spans.

With JDK 11 or later, execute the generated Java against the small
instruction/handler model in `Harness.java`:

```sh
output_dir=$(mktemp -d)
mkdir -p "$output_dir/exceptions"
for cls in AdjacentTry CatchTypes; do
  target/release/radguard decompile --no-metadata \
    tests/fixtures/exception_handlers.dex "Lexceptions/$cls;" \
    > "$output_dir/exceptions/$cls.java"
done
javac -d "$output_dir" "$output_dir"/exceptions/*.java \
  tests/fixtures/exception_handlers/Harness.java
java -Xmx128m -cp "$output_dir" exceptions.Harness
```

This checks 7,875 combinations across nine methods: normal return values,
event order, five exception kinds at protected and unprotected instruction
sites, and exceptions thrown by handler calls. The expected behavior comes
from the Smali handler ranges, independently of the decompiler's graph.

The frozen DAD reference has the same catch-type contamination bug; see
[`../dad_catch_types/README.md`](../dad_catch_types/README.md) for the narrowly
scoped reference corrections and negative comparator tests.
