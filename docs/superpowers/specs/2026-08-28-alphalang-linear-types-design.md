# Alphalang Linear Types And Scheduling Notebook Design

## Goal

Expose Alpha's linear-variable metadata through the `alphalang` Python bindings, enforce the
complete root-level linear analysis from Python, and add an executable notebook that teaches
linearity together with code generation and schedule legality.

This increment does not add new linear semantics. It presents and exercises the semantics already
implemented in `alpha-syntax`, `alpha-model`, `alpha-transform`, and `alpha-codegen`.

## Python API

The compiled `_alpha` extension will expose two new frozen Python types:

```python
alphalang.Multiplicity.LINEAR
alphalang.Multiplicity.UNRESTRICTED

alphalang.Variable(
    name="X",
    domain="[N] -> { X[i] : 0 <= i < N }",
    multiplicity=alphalang.Multiplicity.LINEAR,
)
```

`Variable` instances are immutable snapshots of `alpha_transform::ir::Variable`. They expose:

- `name: str`
- `domain: str`, using the resolved ISL set representation
- `multiplicity: Multiplicity`

`System`, `NormalizedSystem`, and `ScheduledSystem` each expose read-only `inputs`, `outputs`, and
`locals` properties. Each property returns a Python tuple of fresh immutable `Variable` snapshots.
Returning tuples prevents callers from mutating compiler state and preserves the existing
immutable pipeline contract.

`Multiplicity` is a Python enum-like frozen class with the two canonical singleton constants
`LINEAR` and `UNRESTRICTED`. Equality and `repr` are stable and suitable for assertions and
notebook display. The Python package re-exports both new types.

## Root-Aware Parsing

The current binding parses a complete source root but calls `alpha_model::analyze_system` only on
the first system. That bypasses the root-level external and subsystem signature catalog.

`parse_and_lower` will instead:

1. Parse the complete source.
2. Select the first system, preserving current `parse()` return behavior.
3. Run `alpha_model::analyze_root` over the complete root.
4. Reject the source if the selected system has diagnostics, or if whole-program diagnostics were
   attached to that first result.
5. Construct a resolver for the selected system and run its domain analysis before lowering, so
   lowering retains the same initialized resolver state as today.

This makes explicit external signatures and subsystem-call signatures effective through Python
without changing the public `parse()` and `read()` return types. Multi-system selection remains
out of scope.

## Diagnostics And Compatibility

Syntax and semantic failures continue to raise `ValueError`; schedule failures continue to raise
`ScheduleError`. Linear diagnostics use their existing display text, so notebook users see errors
such as non-injective use, overlapping use, unconsumed points, branch mismatch, and incomplete
definition without a second Python-only diagnostic hierarchy.

Existing functions, classes, magics, and generated-code behavior remain compatible. Existing
unrestricted programs report `UNRESTRICTED` metadata and otherwise behave unchanged.

## Notebook

Add `alphalang/notebooks/linear_types.ipynb` as an executed `nbval` fixture. Every cell includes
`metadata.language`; saved cells include stable notebook metadata IDs.

The notebook follows one progression:

1. Parse a pointwise linear transfer and inspect input/output multiplicities.
2. Show exact-once failures for duplicate use, discarded points, and broadcast/non-injective use.
3. Demonstrate an explicit `external move(linear) -> linear` signature.
4. Normalize the valid transfer and generate C.
5. Validate identity and reverse schedules for the independent pointwise statement.
6. Explain that multiplicity analysis precedes scheduling: changing a legal schedule does not
   change which resource points are consumed.
7. Parse a producer-consumer example and show a legal producer-before-consumer schedule.
8. Attempt a reversed producer-consumer schedule and display `ScheduleError`, demonstrating that
   schedule legality preserves true data dependences independently of linearity.

The notebook will keep outputs concise: metadata tuples, selected generated-C lines, and caught
diagnostic messages rather than full compiler dumps.

## Schedule Semantics Explained

The notebook distinguishes two checks:

- **Linear resource validity** is source-semantic and schedule-independent. It verifies exact
  pointwise consumption/production relations before lowering and scheduling.
- **Schedule legality** is execution-order validity. A schedule must be total and injective for
  each statement, use a compatible schedule-space width, reference known statements, and order
  every producer strictly before its consumers.

For `Y[i] = X[i]`, both identity `Y[i] -> [i]` and reverse `Y[i] -> [N - 1 - i]` are legal because
different statement instances have no cross-instance dependence. A non-injective schedule is
invalid even without dependences. For `T[i] = X[i]; Y[i] = T[i]`, any schedule placing `Y[i]`
before its corresponding `T[i]` is illegal because it reverses a real dependence.

## Tests

Binding tests will be added before implementation and observed failing for the missing API or
root-aware behavior:

- `System.inputs`, `outputs`, and `locals` expose names, domains, and multiplicities.
- Metadata is preserved through normalization and scheduling.
- Python parsing accepts an explicit linear external signature when its ports match.
- Python parsing rejects a linear argument passed to a legacy unrestricted external.
- Existing unrestricted binding tests remain green.

Validation commands:

```text
cargo test -p alphalang
uv sync --reinstall-package alphalang
uv run pytest alphalang/tests
uv run pytest --nbval alphalang/notebooks/linear_types.ipynb
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

## Documentation

Update `alphalang/README.md`, `alphalang/notebooks/README.md`, and
`alphalang/python/alphalang/__init__.py` to document and export the metadata API and include the new
notebook in normal test commands.

## Non-Goals

- Structured Python diagnostic objects.
- Selecting among multiple systems from one source string.
- Mutating multiplicity or domains from Python.
- Scheduling subsystem use equations, which remain unsupported by the code generator.
- Adding new built-in linear operator signatures.
