# alphac

The Alpha language CLI compiler — wires up [`alpha-syntax`](../alpha-syntax),
[`alpha-model`](../alpha-model), [`alpha-transform`](../alpha-transform), and
[`alpha-codegen`](../alpha-codegen) end to end. Replaces the source project's `alpha.loader`,
deliberately without its accidental coupling to the schedule-tree grammar.

## Usage

```
alphac <file.alpha> [-o <file.c>]
```

Without `-o`, generated C is printed to stdout.

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
`alpha.codegen.tests` reference fixtures (see `alpha-codegen`'s README). No dedicated crate-level
test yet — see `docs/progress.md`'s "immediate next steps" for what that would need
(a C compiler on the test machine to actually compile the generated output).
