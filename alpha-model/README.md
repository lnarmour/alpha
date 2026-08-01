# alpha-model

Semantic model and six-phase checker for the Alpha language: name/domain resolution, expression-
and context-domain inference, and the well-formedness diagnostics, all built over
[`alpha-syntax`](../alpha-syntax)'s typed AST and [`isl`](../isl)'s polyhedral types.

This crate ports the source project's `AlphaInternalStateConstructor` six-phase pipeline as six
ordinary Rust passes — exhaustive `match` on the expression enum replaces the Java visitor pattern
the original uses purely for double-dispatch, one of the places this port comes out simpler than
the source, not just equivalent.

## The six phases

| Phase | Module | What it does |
|---|---|---|
| 1 | `resolve` | Interface resolution: system parameter domains, variable domains (with comma-list sibling inheritance and cycle detection), `RectangularDomain` expansion, named-constant substitution |
| 2 (partial) | `function`, `value` | `Function`/`ArrayFunction` → `MultiAff` resolution; `ArrayPolynomial` → `PwQPolynomial`; the calculator's dynamic `Set`/`Map`/`Function`/`Polynomial` type tag and its unary/binary operator evaluator |
| 3 | `domain` | Bottom-up expression-domain inference (`Resolver::expression_domain`) |
| 4 | `domain` | Top-down context-domain inference (`Resolver::context_domain`, driven via `equation_context_domains`) |
| 5 | `uniqueness` | Duplicate name checks — variables/`define`d objects, equation targets, constants, systems, external functions |
| 6 | `completeness` | The well-formedness catalog: incomplete/overlapping system bodies and equations, unbounded reductions, undefined variables, use-equation recursion/output-completeness |

Supporting modules: `diagnostic` (the closed `Diagnostic` enum every phase reports through),
`context_names` (shared raw-text helpers for reading a construct's own bound names off its
captured text), `walk` (a generic recursive-descent visitor factored out once phases 5 and 6 both
needed the same "find every node of kind X" traversal), and `analyze` (`analyze_root`/
`analyze_system` — the consolidated entry point that wires all six phases together for callers
like `alphac`).

## Deliberate, documented scope boundaries

All six phases exist and are fixture-tested against the full 82-file corpus, but three gaps are
left deliberately open rather than guessed at:

- **Convolution's own expression/context domain** (`domain.rs`) — needs isl vertex enumeration,
  not yet bound in `isl-sys`. Its kernel/data sub-expressions are still walked and recorded, just
  not the convolution node itself.
- **`UseEquation`'s context domain** (`domain.rs`) — needs the cross-system instantiation-domain
  extension the source system's `AlphaExpressionUtil.extendCalleeDomainByInstantiationDomain`
  implements; only `StandardEquation` bodies get phase 4 today.
- **`check_use_equation_recursion`'s self-recursion detection** (`completeness.rs`) compares the
  callee's bare name against the enclosing system's own name rather than resolving to an actual
  system object (no whole-program symbol table exists for this). Sound for the common case; would
  miss a self-call written via a full package-qualified path.

`check_use_equation_outputs` (`completeness.rs`) is a faithful port but stays **dormant** until the
`UseEquation` context-domain gap above closes.

See each module's own doc comment, and `docs/progress.md` in the workspace root, for the full rationale and
the fixture programs that motivated each rule.

## Status

Done for all six phases. Fixture-tested against all 82 `.alpha` fixtures from the sibling
`alpha-language` repo:

- `resolve_fixtures` — 271 systems / 665 variables resolve
- `function_fixtures` — 469 dependence/reduce functions resolve
- `domain_fixtures` — 345 equations' expression domains + 1710 context-domain entries
- `uniqueness_fixtures` — 271 systems, zero false positives, + 7 unit tests
- `completeness_fixtures` — 224 `src-valid` systems, zero false positives, + 5 unit tests
  confirming real diagnostics on known-invalid fixtures

## Two rules worth knowing before touching `domain.rs`

Both found by fixture-testing against real programs, not just reading the Java source:

- `RestrictExpression`'s explicit-tuple domain and `SelectExpression`'s relation **replace** the
  ambient index-name context for their sub-expression (once dimension counts match) — they do
  *not* extend it. Only `ConvolutionExpression`'s kernel domain genuinely extends.
- A bare `{: constraints}` domain is parsed as the `DOMAIN` node kind when it's a
  `RestrictExpression`'s domain source, but as `ARRAY_DOMAIN` everywhere else (`when`/`else`
  guards) — `is_bare_colon_domain` detects the shape by content, not node kind.
