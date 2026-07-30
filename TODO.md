# Progress notes — Rust port of alpha-language

Status as of this pause. For the *why* behind every architectural choice, see
`docs/rust-port-design.md` — this file is the *where things actually stand* companion to that
design doc, meant to let a new session pick up cold.

## TL;DR

Lexer → parser → typed AST is done and thoroughly fixture-tested. The isl FFI bindings and safe
wrapper are done and fixture-tested. `alpha-model`'s semantic analysis is ~1/3 done: interface
resolution (phase 1) and function-literal resolution (phase 2, partial) both work and are
fixture-tested against all 82 real `.alpha` programs from the sibling `alpha-language` repo.
Nothing in `alpha-transform`, `alpha-codegen`, `alphac`, or the VS Code extension exists yet
beyond crate stubs.

Whole workspace builds clean, clippy clean, zero test failures, as of this pause.

## Environment setup (do this first in a fresh session/machine)

- **Rust**: installed via `rustup` (not system package manager). If a fresh machine: `curl
  --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`, then `source
  "$HOME/.cargo/env"`.
- **isl + pkg-config**: installed via Homebrew (`brew install isl pkg-config`) — isl 0.28, MIT
  licensed. `isl-sys`'s `build.rs` uses `pkg-config` to find it; **`pkg-config` is installed to
  `/opt/homebrew/bin`, which is often not on `PATH` in non-interactive shells** — prefix cargo
  invocations with `export PATH="/opt/homebrew/bin:$PATH"` if `cargo build` can't find isl.
- **libclang** (for `bindgen`): comes with Xcode Command Line Tools, already present on this
  machine (`/Library/Developer/CommandLineTools/usr/lib/libclang.dylib`).
- **Working directory matters**: this repo (`~/git/poly/alpha-rs`) is *not* nested under the
  `alpha-language`/`alphaz` Java repos — it's a sibling directory, its own git repo. Cargo
  commands need `cwd` to actually be inside `alpha-rs` (a stray `cd` elsewhere earlier in a
  session has bitten us once already — if `cargo build` says "could not find Cargo.toml", check
  `pwd`).
- **Git**: the user handles `git add`/`commit`/`status` themselves — don't run those proactively.

Typical command line for this session:
```
cd ~/git/poly/alpha-rs
source "$HOME/.cargo/env" && export PATH="/opt/homebrew/bin:$PATH"
cargo test --workspace
```

## Fixture corpus

All conformance tests read `.alpha` fixtures directly from the sibling Java repo at
`~/git/poly/alpha-language/tests/**` (82 files total, both `src-valid` and `src-invalid`
subtrees) via a relative path (`../../alpha-language/tests` from each crate's `CARGO_MANIFEST_DIR`).
Tests skip gracefully (with an `eprintln!`) if that directory isn't present, so the suite still
runs (with reduced coverage) if this repo is ever moved somewhere without that sibling checkout.

**Important, hard-won finding**: despite one subdirectory being named `src-invalid/syntax-tests`,
*every* fixture in the entire corpus is syntactically well-formed Alpha — including that one.
Reading them confirmed they all test *semantic* violations (dimension mismatches, duplicate
definitions, recursive calls, undeclared array-notation indices), not grammar errors. Don't
assume a `src-invalid` path means "should fail to parse."

## Crate-by-crate status

| Crate | Status | Tests |
|---|---|---|
| `alpha-syntax` | **Done**: lexer, rowan CST, resilient recursive-descent/Pratt parser, typed `ast::` layer | 7 unit + `lex_fixtures` (82 files, 0 lex errors) + `parse_fixtures` (82 files, 0 syntax errors) + `ast_fixtures` (271 systems/345 equations walked) + `resilience` (hand-picked garbage + every ~17-byte truncation of every fixture, zero panics) |
| `isl-sys` | **Done**: bindgen FFI over a bounded isl header set (set/map/aff/constraint/polynomial/ast_build/id/printer/options) | 1 runtime smoke test |
| `isl` | **Done**: safe wrapper — `Context`, `Set`/`BasicSet`, `Map`, `Aff`/`MultiAff`, `Constraint`/`LocalSpace`, `PwQPolynomial`, `AstBuild`/`AstNode`/`AstExpr`, `Space`/`DimType` | 9 integration tests (set/map algebra, hulls, gist, dependence image/preimage, constraint construction, AST-builder generating real nested for/if C code) |
| `barvinok-sys` / `barvinok` | Stub only (intentionally deferred — GPL, feature-gated, not needed until `alpha-codegen`'s cardinality counting) | none |
| `alpha-model` | **Partial**: phase 1 (interface resolution) done; phase 2 (function-literal resolution) done; phases 3–6 not started | `resolve_fixtures` (271 systems/665 variables resolve) + `function_fixtures` (469 dependence/reduce functions resolve) |
| `alpha-transform` | Stub only | none |
| `alpha-codegen` | Stub only | none |
| `alphac` | Stub only (prints a placeholder message) | none |
| VS Code extension | Doesn't exist yet | — |

## `alpha-model` in detail — what exists, what's next

Files: `alpha-model/src/{diagnostic,value,resolve,function}.rs`.

- **`diagnostic.rs`**: the closed `Diagnostic` enum (per your decision to keep it closed). Current
  variants: `Syntax`, `IslError`, `InvalidCalculatorOperand(Pair)`, `UnsupportedCalculatorOp`,
  `UndefinedReference`, `CyclicDefinition`. More will be added *as* each check that produces them
  gets implemented (phase 6 will add most of the remaining ~25 or so from the source project's
  `AlphaIssueFactory` catalog) — don't add unused variants speculatively.
- **`value.rs`**: `Value` (the calculator's dynamic `Set`/`Map`/`Function`/`Polynomial` type tag)
  and the unary/binary calculator-operator evaluator. Deliberately partial: `cross` (`flatProduct`)
  only implemented for `Map`×`Map`; `Set`×`Set` cross product reports
  `Diagnostic::UnsupportedCalculatorOp` rather than guessing at an ambiguous isl equivalent.
- **`resolve.rs`**: `Resolver<'a>`, one per `System`. Phase 1: `param_domain()`,
  `variable_domain(name)` (comma-list inheritance via next-sibling lookahead, cycle-detected),
  `RectangularDomain` expansion, named-constant substitution (`text_of`, token-aware, walks up to
  enclosing `Root`/`AlphaPackage` for `constant NAME=INT` declarations). Deliberately scoped to
  "no ambient equation-local index names" — see the module doc for why phase 2 is split out.
- **`function.rs`**: `Resolver::eval_function` — resolves `Function`/`ArrayFunction` calculator
  literals into real `isl::MultiAff`, given an explicit `index_names: &[String]` context the
  *caller* computes. This crate does **not** yet maintain the source system's full
  `contextHistory` stack itself (pushed/popped at `RestrictExpression`/`SelectExpression`/
  `AbstractReduceExpression`/`ConvolutionExpression`/`UseEquation`) — each caller is responsible
  for extending context correctly at those points. The `function_fixtures.rs` test had to work
  out (and now documents in comments) the exact scoping rules for each construct; **read that
  test file before implementing phases 3/4**, since it's the closest thing to a spec for how
  context should be threaded that currently exists in this codebase:
  - `ArrayFunction` (`[k]`) sugar *extends* ambient context.
  - `Function` (`(i,j->...)`) *replaces* ambient context outright (self-declaring).
  - `ConvolutionExpression`'s kernel domain and `SelectExpression`'s relation range both
    introduce new bound names for their sub-expression.
  - `UseEquation`'s `over` clause *and* `with` clause both contribute names (not just `with`).
  - Extending context must dedupe (a construct can re-declare a name already in scope, e.g. a
    `RestrictExpression`'s own domain reusing the enclosing equation's index name) — isl rejects
    a tuple with a repeated name.

**Not yet built**: phases 3–4 (expression-domain / context-domain inference over full equation
bodies — the bottom-up/top-down passes from `ExpressionDomainCalculator`/`ContextDomainCalculator`
in the source Java), phase 5 (name uniqueness), phase 6 (the ~30-diagnostic well-formedness
catalog: incomplete/overlapping system bodies, incomplete/overlapping equations, undefined
variables, unbounded reductions, etc. — see `docs/rust-port-design.md` §6 for the full list from
the source project's `UniquenessAndCompletenessCheck`).

## Non-obvious bugs found and fixed this session (worth knowing before touching the parser)

All in `alpha-syntax/src/parser/`:

1. **Trivia-attachment timing.** `start_node` must flush pending trivia *except* on the very
   first call (which opens `ROOT`); `finish_node` must *never* flush (trailing trivia belongs to
   whichever ancestor's next `bump()` picks it up, not the node that happens to be closing) —
   `ROOT`'s own closing is the one exception, handled with an explicit `flush_trivia()` call in
   `items::root` right before its `finish_node()`. Getting this wrong doesn't break parsing, it
   silently corrupts node *text* (e.g. a `RectangularDomain` node's `.text()` including leading/
   trailing whitespace it shouldn't) — the kind of bug that only shows up once something reads
   raw node text (as `alpha-model` does pervasively), not in parser-level tests.
2. **Lexical errors must stay in the token stream** (`parser::Raw`/`RawKind`), not be dropped —
   otherwise a byte the lexer can't tokenize (e.g. inside an unterminated comment) silently
   vanishes from the tree, breaking losslessness.
3. `ParamDomain::param_names()` / `RectangularDomain::index_names()` (in `ast.rs`) must stop at
   the right delimiter (`->` / `as`) — these nodes have no wrapper node around their `{...}`
   body or bound-list (raw-captured directly as sibling tokens), so a naive "all IDENT children"
   collection also picks up identifiers used *inside* the constraint/bound text.

And in `isl`/`isl-sys`:

4. `isl_set_read_from_str` requires an explicit `[params] -> ...` prefix — it does not infer free
   parameters in a bare `{...}` set literal, unlike the source Java system's approach.
5. Alpha's `{}` empty-domain shorthand isn't valid to isl directly (`isl_set_read_from_str("{}")`
   fails an internal assertion) — needs normalizing to `{ : }` first.
6. `isl_multi_aff_read_from_str` requires the `[in]->[out]` pair wrapped in literal `{ }` braces,
   even with no parameter prefix.
7. isl prints errors to stderr by default (`ISL_ON_ERROR_WARN`) in addition to recording them —
   set `ISL_ON_ERROR_CONTINUE` in `Context::new()` so an IDE-facing tool doesn't spam stderr on
   every invalid keystroke.
8. Cycle-detection state (`Resolver`'s `defined_state`/`variable_state` maps) must be cleared on
   a *failed* resolution, not just a successful one — otherwise a real (unrelated) error on first
   lookup poisons every later lookup of the same name into a false `CyclicDefinition` report.

## Immediate next steps (in order)

1. Phases 3–4: expression-domain / context-domain inference over full equation bodies. Read
   `function_fixtures.rs`'s context-tracking logic first (see above) — it's the de facto spec.
2. Phase 5: name uniqueness (duplicate systems/variables/constants/external-functions).
3. Phase 6: the well-formedness catalog (§6 of the design doc has the full list).
4. `alpha-transform`: `Normalize` + `NormalizeReduction` only (per scope — see design doc §0/§7).
5. `alpha-codegen`: `simpleC` model + `WriteC` demand-driven generator (design doc §7).
6. `alphac` CLI wiring it all together.
7. VS Code extension (napi-rs native addon + TextMate grammar — design doc §8).

## Where to look for more context

- `docs/rust-port-design.md` — the full design doc (scope, naming conventions, crate layout,
  parsing strategy, ISL binding strategy + licensing, codegen plan, VS Code architecture,
  phased roadmap). Read this for *why*, not just *what*.
- `~/.claude/projects/-Users-anna-git-poly/memory/` — cross-session memory (currently just one
  entry: don't run `git add`/`commit`/`status` proactively in this repo).
