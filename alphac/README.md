# alphac

The Alpha language CLI compiler — wires up [`alpha-syntax`](../alpha-syntax),
[`alpha-model`](../alpha-model), [`alpha-transform`](../alpha-transform), and
[`alpha-codegen`](../alpha-codegen) end to end. Replaces the source project's `alpha.loader`,
deliberately without its accidental coupling to the schedule-tree grammar.

## Usage

```
alphac <file.alpha> [-o <file.c>] [--wrapper]
```

Without `-o`, generated C is printed to stdout.

`--wrapper` additionally emits a `*_wrapper.c` test harness per system (via
[`alpha_codegen::generate_wrapper`](../alpha-codegen)) alongside `-o`'s own output file — allocates
memory for every parameter, calls the generated function, and frees it, so a generated system can
be compiled and run without hand-written boilerplate. The wrapper is written in the same directory
as `-o` and named after its stem: `-o dir/foo.c --wrapper` writes `dir/foo_wrapper.c` (or, for a
file with more than one system, one `dir/foo_<SystemName>_wrapper.c` per system). Requires `-o`:
with no output file there's no path to derive the wrapper's name/location from, so `--wrapper`
alone is a hard error.

## Pipeline

Per system found in the input file (walking nested `AlphaPackage`s), in order:

```
parse (once, for the whole file)
  -> analyze (all six alpha-model phases, via alpha_model::analyze_system)
  -> if clean: lower -> NormalizeReduction -> Normalize(deep=true) -> alpha_codegen::generate_system
  -> print
```

A file with more than one system gets one self-contained generated-C block per system,
concatenated — each block carries its own `#include`/macro preamble.

Diagnostics (syntax errors, semantic-analysis diagnostics, codegen errors) are printed to stderr;
the process exits non-zero if any system fails to generate.

## Cardinality counting (`barvinok` feature)

Forwards to `alpha-codegen`'s `barvinok` feature (off by default) — see
[that crate's README](../alpha-codegen). The default build here, without the feature, is what
ships.

## Status

Done for scope: CLI wiring is complete and manually verified against the three
`alpha.codegen.tests` reference fixtures (see `alpha-codegen`'s README). `--wrapper` generation is
implemented and covered by dedicated crate-level tests (`tests/wrapper_cli.rs`): a wrapper file is
written next to `-o`'s path, and `--wrapper` without `-o` fails with a clear error. No dedicated
crate-level test of the main codegen path yet — see `docs/progress.md`'s "immediate next steps" for
what that would need (a C compiler on the test machine to actually compile the generated output).
