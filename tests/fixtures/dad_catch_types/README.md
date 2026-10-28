# Frozen catch-type annotations

DAD renders a catch declaration from the inferred type of a reusable
variable. Later assignments can change it to `boolean`, `String`, or even an
array type. Its `MoveExceptionExpression.type` still records the original
exception type. Comparing the corrected Rust declaration against that broken
text without correcting the reference would reject the intended fix.

The quality gate reads the original DEX exception table and the leading
`move-exception` register. It corrects only reference declarations when all
handlers have one type, or when every handler using a register has exactly
the same type group. A split DAD name such as `v5_2` still identifies register
5. Type groups from different try regions remain separate; a shared register
with different handler types is ambiguous.

`classes5.json` supplies render-order annotations for 77 mixed-type methods
in the existing `classes5.dex` fixture. These were collected from DAD's
original exception-instruction metadata, not from RadGuard's output. The
generator requires its complete method body to match the frozen DAD source
exactly before using that metadata. The JSON is bound to the DEX SHA-256,
class and method index, exact method-body SHA-256, and catch count. During
every quality check, each annotated type must also occur in the original
DEX handler table. A changed source, DEX, or count raises an error.

These corrections never change candidate catch types. A `null` entry means
there is no preserved exception instruction, so that declaration is left
alone. The existing type-normalization fallback is skipped for annotated
declarations so an incorrect candidate type or order remains a mismatch. The gzip reference caches,
thresholds, and class whitelist are unchanged. Comparator tests cover wrong
candidate types, swapped handlers, unrelated body changes, array types in
broken references, ambiguous registers, and stale annotations.

Normal checks need only Python's standard library:

```sh
python3 -S -m unittest discover -s tests -p test_quality_gate.py
```

To reproduce the selected annotations with an ASC/DAD source installation
and its Python dependencies available:

```sh
python3 tests/regenerate_catch_annotations.py \
  --dadguard-source /path/to/ASC \
  --dex tests/fixtures/classes5.dex \
  --annotations tests/fixtures/dad_catch_types/classes5.json
```

The initial reproduction used ASC revision
`464f9fef0db41099c4cf3f55c78b33f2606780d2`. The script consults no Rust binary;
running it reproduced the checked-in JSON byte-for-byte. It is an optional
maintenance tool and does not enable live DAD generation in the quality gate.
