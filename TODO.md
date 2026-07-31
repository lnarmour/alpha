# Progress notes — Rust port of alpha-language

Status as of this pause. For the *why* behind every architectural choice, see
`docs/rust-port-design.md` — this file is the *where things actually stand* companion to that
design doc, meant to let a new session pick up cold.

## TL;DR

Lexer → parser → typed AST is done and thoroughly fixture-tested. The isl FFI bindings and safe
wrapper are done and fixture-tested. **All six phases of `alpha-model`'s semantic analysis exist**
and are fixture-tested against all 82 real `.alpha` programs from the sibling `alpha-language`
repo — see below for the handful of deliberate, documented scope boundaries phases 2–4 and 6 stop
short of. **`alpha-transform` exists too**: `Normalize` (the ~25-rule term-rewriting pass) and
`NormalizeReduction`, operating on a new owned "resolved AST" this crate introduces (see
`alpha-transform/src/ir.rs`'s doc for why — the syntax layer's lossless rowan CST isn't something
a rewrite pass should mutate in place). Every equation that successfully lowers, across the whole
82-fixture corpus, reaches the source system's documented normal form.

**`alpha-codegen`/`alphac` now exist end-to-end too**: the `simpleC` model, the `WriteC`
demand-driven generator (memoized `eval_<Var>` functions, synthesized `reduce<N>` helpers, and the
system's own driver entry point — all built on `isl_ast_build` for loop generation and isl's own
C-format printer for every affine/boolean condition, never a hand-written affine-to-C converter),
and `alphac`'s CLI wiring (`parse → analyze → lower → NormalizeReduction → Normalize → generate →
print`). Validated two ways: (1) generated C for `CopyInput`/`PrefixScan`/`LUDecomposition` (the
three `alpha.codegen.tests` reference fixtures) compiles warning-free and, linked against the
sibling Java system's own `*-wrapper.c`/`*_verify.c` files, **passes real numeric verification**
against the original AlphaZ-generated output across a range of `N`; (2) a corpus-wide fixture test
runs the whole pipeline over all 82 fixtures and asserts every system either generates successfully
or hits one of this session's documented, named scope boundaries — zero unexpected failures. The
VS Code extension is the one remaining piece with nothing built yet beyond a design (§8).

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
| `alpha-model` | **All six phases exist** (phase 2 partially — see below; a few deliberate scope boundaries in phases 3–4/6 — see below) | `resolve_fixtures` (271 systems/665 variables resolve) + `function_fixtures` (469 dependence/reduce functions resolve) + `domain_fixtures` (345 equations' expression domains + 1710 context-domain entries) + `uniqueness_fixtures` (271 systems, zero false positives, + 7 unit tests) + `completeness_fixtures` (224 `src-valid` systems, zero false positives, + 5 unit tests confirming real diagnostics on known-invalid fixtures) — all across all 82 fixtures |
| `alpha-transform` | **Done for scope**: `Normalize` + `NormalizeReduction` on a new owned IR (`ir.rs`/`lower.rs`/`normalize.rs`/`normalize_reduction.rs`) — see below | `normalize_fixtures` (428 equations, across every fixture that lowers, all reach the documented normal form) |
| `alpha-codegen` | **Done for scope**: `simplec.rs` (the C AST) + `layout.rs` (storage layout) + `writec.rs` (the generator) — see below | `codegen_fixtures` (199/220 systems with zero analysis diagnostics generate C; the other 21 hit a *named* documented scope boundary — zero unexpected failures) + functional verification of `CopyInput`/`PrefixScan`/`LUDecomposition` against the sibling Java system's own reference `_verify.c` files (see below) |
| `alphac` | **Done for scope**: CLI wiring `parse → analyze → lower → NormalizeReduction → Normalize → generate → print`, `-o` output flag | manually verified against the three reference fixtures (see below) — no dedicated crate-level test yet |
| VS Code extension | Doesn't exist yet | — |

## `alpha-model` in detail — what exists, what's next

Files: `alpha-model/src/{diagnostic,value,resolve,function,context_names,domain,uniqueness,walk,completeness}.rs`.

- **`diagnostic.rs`**: the closed `Diagnostic` enum (per your decision to keep it closed), now with
  every variant phases 1–6 need: `Syntax`, `IslError`, `InvalidCalculatorOperand`,
  `InvalidCalculatorOperandPair`, `UnsupportedCalculatorOp`, `UndefinedReference`,
  `CyclicDefinition`, `IncompatibleContextAndExpressionDomain`, `AutoRestrictNotInCase`,
  `MultipleAutoRestrict`, `EmptyAutoRestrict`, `MultipleUnrestrictedSystemBody`,
  `RestrictDomainDimensionMismatch`, `SelectRelationDimensionMismatch`, `DuplicateSystem`,
  `DuplicateExternalFunction`, `DuplicateVariable`, `DuplicatePolyhedralObject`,
  `DuplicateStandardEquation`, `DuplicateUseEquation`, `DuplicateAlphaConstant`,
  `EmptySystemBody`, `OverlappingSystemBodies`, `IncompleteSystem`, `IncompleteEquation`,
  `OverlappingCaseBranch`, `UnboundedReductionBody`, `InfinitelyRecursiveUseEquation`,
  `OverlappingUseEquations`, `IncompleteUseEquation`, `UndefinedVariable`. Add more only if a
  genuinely new check needs one — don't add unused variants speculatively.
- **`value.rs`**: `Value` (the calculator's dynamic `Set`/`Map`/`Function`/`Polynomial` type tag)
  and the unary/binary calculator-operator evaluator. Deliberately partial: `cross` (`flatProduct`)
  only implemented for `Map`×`Map`; `Set`×`Set` cross product reports
  `Diagnostic::UnsupportedCalculatorOp` rather than guessing at an ambiguous isl equivalent.
- **`resolve.rs`**: `Resolver<'a>`, one per `System`. Phase 1: `param_domain()`,
  `variable_domain(name)` (comma-list inheritance via next-sibling lookahead, cycle-detected),
  `RectangularDomain` expansion, named-constant substitution (`text_of`, token-aware, walks up to
  enclosing `Root`/`AlphaPackage` for `constant NAME=INT` declarations). Deliberately scoped to
  "no ambient equation-local index names" — see the module doc for why phase 2 is split out.
- **`function.rs`**: `Resolver::eval_function` (`Function`/`ArrayFunction` → `MultiAff`) and
  `Resolver::eval_polynomial_in_context` (`ArrayPolynomial` → `PwQPolynomial`, handling the
  `;`-separated piecewise case — each piece needs its own synthesized `[ctx] ->` prefix, not one
  prefix around the whole thing). Each caller computes and threads the ambient `index_names`
  context itself; see `context_names.rs`/`domain.rs` for the real scoping rules now in force
  (superseding this file's now-stale-in-spirit "read `function_fixtures.rs` before implementing
  phases 3/4" note — that test's own context-tracking copy was validated only for "does
  `eval_function` not error," which turned out to accept some wrong-but-parseable contexts; the
  real rules, worked out against the whole corpus while building phase 4, are in `domain.rs`'s
  module doc and doc comments on `restrict_domain`/`select_relation`/`reduce_projection`).
- **`context_names.rs`**: shared raw-text helpers (`domain_tuple_names`, `relation_range_names`,
  `bare_identifier_elements`, `extend_unique`) for reading a construct's own bound names off its
  captured text — used by both `function.rs`'s callers and `domain.rs`.
- **`domain.rs`** (phases 3–4, new this session): `Resolver::expression_domain` (bottom-up) and
  `Resolver::context_domain` (top-down, private — drive it via `equation_context_domains`), plus
  `equation_expression_domains`/`equation_context_domains` as the per-equation entry points. Two
  **deliberate, documented gaps** (see the module doc for the full rationale): convolution's own
  expression/context domain (needs isl vertex-enumeration, not bound in `isl-sys` yet — its
  kernel/data sub-expressions are still walked and recorded, just not the convolution node
  itself), and `UseEquation`'s context domain (needs the cross-system instantiation-domain
  extension from `AlphaExpressionUtil.extendCalleeDomainByInstantiationDomain` — genuinely more
  involved; only `StandardEquation` bodies get phase 4 for now).
  **Two rules worth knowing before touching this file, both found by fixture-testing against
  real corpus programs (`array1e`/`array2`/`PrefixScan`/`LUDecomposition`/`polynomial2` etc.),
  not just reading the Java source**:
  - `RestrictExpression`'s explicit-tuple domain (`{[x]:...}`) and `SelectExpression`'s relation
    *replace* the ambient index-name context for their sub-expression (once dimension counts
    match) — they do **not** extend it. Only `ConvolutionExpression`'s kernel domain genuinely
    extends. (The source Java's `inRestrictExpression`/`inSelectExpression` make this explicit;
    `function_fixtures.rs`'s own context-tracking, written earlier for a narrower purpose, used
    `extend_unique` for both and happened to not get caught by that test because the wrong context
    still produces a syntactically-parseable — just semantically wrong — `MultiAff`.)
  - A bare `{: constraints}` domain (no explicit `[...]` tuple) is parsed as the `DOMAIN` node
    kind when it's a `RestrictExpression`'s domain source, but as `ARRAY_DOMAIN` everywhere else
    (`when`/`else` guards) — `is_bare_colon_domain` detects the shape by content, not node kind.
    Its constraints refer to the *ambient index names directly* (not parameters), needing them
    synthesized as its implicit tuple — same idea as `ArrayFunction`'s implicit input tuple, but
    for a bare domain instead of a bare function.

- **`uniqueness.rs`** (phase 5, new this session): `check_system_uniqueness` (duplicate
  variable/`define`d-object names — one shared namespace; duplicate `StandardEquation`/
  `UseEquation` targets within a `SystemBody`, skipping the legal case of multiple `UseEquation`s
  alone writing to the same variable; duplicate `constant` declarations) and
  `check_program_uniqueness` (duplicate systems/external functions by fully-qualified name across
  every `Root` given, folding `check_system_uniqueness` in for each system found — mirrors the
  source's own `check(List<AlphaRoot>)` composition). Returns `Vec<Diagnostic>` directly (not
  `Result`) since, unlike every earlier phase, a program can have several unrelated duplicates and
  the source system reports all of them in one pass rather than failing fast.
  **Deliberate divergence worth knowing**: the duplicate-`constant` check only looks at a system's
  *direct* container (immediate `AlphaPackage`/`Root`), not the full ancestor chain
  `resolve.rs`'s `constants_in_scope` walks for value resolution — matches the source system's
  actual `AlphaUtil.getAlphaConstants` exactly, and avoids false positives on legitimate constant
  shadowing across nested packages. `resolve.rs`'s broader walk is unchanged (out of this phase's
  scope, still fixture-validated as-is, and no real fixture nests packages deeply enough to expose
  the difference) — flagged as a latent inconsistency between the two, not fixed.

- **`walk.rs`**: `walk_expr`, a generic recursive-descent visitor over every `Expr` node beneath a
  given one — factored out once `uniqueness.rs` and `completeness.rs` both needed the identical
  "find every node of kind X anywhere in this subtree" traversal (mirrors the source system's
  `EcoreUtil.getAllContents`/`getAllContentsOfType`).
- **`completeness.rs`** (phase 6, new this session): `check_system_bodies` (overlapping/empty
  `SystemBody` domains, incomplete system coverage), `check_standard_equation_completeness`
  (an equation's expression domain must cover its variable's domain), `check_case_branches`
  (disjoint `case` branch context domains), `check_reduce_bounded` (a `ReduceExpression`'s body
  must range over a boundable index space — new isl bindings: `Set::eliminate`/`Set::params`),
  `check_use_equation_recursion` (self-recursion via an identity call-parameter function — new
  isl bindings: `MultiAff::move_dims`, `Map::is_identity`), `check_use_equation_outputs`
  (`UseEquation`s targeting one variable must have disjoint, jointly-complete instantiation
  domains), `check_undefined_variables` (every output, and every referenced local, needs a
  defining equation — pure syntax, no isl). All return `Vec<Diagnostic>` like phase 5, for the
  same reason (a program can have several unrelated problems). `Resolver::analyze_system` (added
  to `domain.rs`) runs phases 3–4 across a whole system into one shared pair of domain maps, since
  phase 6's whole-system checks need every equation's domains available together.
  **Two things worth knowing before touching this file**:
  1. `check_system_bodies` intersects each body's raw `when`-guard domain with the *system's own*
     parameter domain before checking pairwise disjointness — found via a real false positive on
     `FFT.alpha`: its `N%2=1`/`N<=2` guards both admit `N=1` as raw text, which isn't a real
     conflict since the system itself only declares `N>=2`. Skipping this intersection makes the
     disjointness check too strict for any system whose guards are only pairwise-disjoint *within*
     its own declared domain (the overwhelmingly common case) rather than unconditionally.
  2. `check_use_equation_outputs` is a faithful port, but is *dormant* (never actually fires)
     until `domain.rs` grows a `UseEquation` context domain (see that module's gap) — it inherits
     the source system's own graceful "skip this variable's check if any of its
     `VariableExpression`s don't have a context domain yet" behavior, and every `UseEquation`
     internal expression always hits that path today. Written now anyway so it needs no changes
     once that gap closes — see `subsystem1.alpha`'s `subsystem1a`/`subsystem1b` fixtures for real
     examples this would catch once it's live.
  3. `check_use_equation_recursion`'s self-recursion detection compares the callee's bare name
     against the enclosing system's own name rather than resolving to an actual system object —
     this port has no whole-program symbol table (`uniqueness.rs`'s program-wide check was built
     for a different purpose). Sound for the common case; would miss a self-call written with a
     full package-qualified path. Flagged in the module doc, not silently guessed at.

## `alpha-transform` in detail — what exists, what's next

Files: `alpha-transform/src/{ir,lower,normalize,normalize_reduction}.rs`.

- **`ir.rs`**: a new, owned, mutable "resolved AST" (`System`/`SystemBody`/`Equation`/`Expr`) —
  deliberately *not* built on `alpha_syntax::ast`'s rowan CST, since `Normalize` is a term-rewriting
  pass that needs to replace nodes and recompute attached domains as it goes, and rowan's tree is a
  persistent/structurally-shared one meant for the opposite property (cheap lossless *parse-time*
  edits). `Expr` carries its own `expression_domain: Set` and `context_domain: Option<Set>`
  directly as fields (no side-table). This was a genuine, up-front architecture decision — see the
  design doc discussion before this work started for the alternatives considered (full port vs.
  core subset vs. codegen-first) and why "full port on a new owned IR" was chosen.
- **`lower.rs`**: builds an `ir::System` from an analyzed `ast::System` (via
  `alpha_model::domain::Resolver::analyze_system`) by *transcribing*, never re-deriving — every
  isl object a node needs comes from the exact same now-`pub` `Resolver` methods phases 3–4 already
  used (`restrict_domain`, `select_relation`, `reduce_projection`, `convolution_kernel_names`,
  `eval_function`, `eval_calc_expr`), so lowering can never disagree with analysis about a node's
  function/context. Equations that don't lower (a `ConvolutionExpression`'s own domain — the
  `domain.rs` gap — or a fuzzy feature) are skipped, each contributing one diagnostic; the rest of
  the system still lowers. One syntax-layer subtlety worth knowing: `val(f)` (function-valued) and
  `val{...}` (polynomial-valued) share one `INDEX_EXPR` syntax node kind, but only the
  function-valued shape has a `Normalize` rewrite rule in the source system — lowering preserves the
  distinction as two different `ir::ExprKind` variants (`IndexFunction`/`IndexPolynomial`) so
  `normalize.rs` doesn't need to re-derive it.
- **`normalize.rs`**: the ~25-rule `Normalize` port. Structural rewriting always recomputes a
  rewritten node's `expression_domain` immediately and correctly (`expr_from_kind`, the same
  formulas as `alpha_model::domain`'s phase 3, just run directly on isl objects already sitting in
  the tree); `context_domain` is recomputed in a separate top-down pass (`refresh_context`, mirrors
  phase 4) run between rounds of structural rewriting (`apply`'s `MAX_ROUNDS = 8` loop), since two
  rules (case-branch pruning by empty context, the binary-case cross-product) need it fresh. See
  the module doc for the two small, deliberate departures from the source's literal behavior
  (symmetric binary-restrict-hoist; unconditional redundant-restrict removal) — both make this port
  strictly more complete, never differently correct.
  **The two hardest-won bugs this session, both silent-wrong-answer bugs, not compile errors —
  worth real attention before extending this file**:
  1. Every "no rule matched, put this node back unchanged" fallback path reconstructs via
     `expr_from_kind`, which (correctly, for a genuinely *new* node) always sets
     `context_domain: None`. Applied to an *unchanged* node, this silently wipes context on every
     single node in the tree on every structural pass — including case branches, right before their
     parent needs to read it — which looks like "context is never available" no matter how many
     refresh rounds run, since children are always processed (and thus wiped) before their parent's
     own rules see them in the same pass. Fixed in `normalize_expr`/`normalize_dependence_operand`
     themselves: when `try_rewrite` reports "unchanged" (`Err`), restore the *original* node's
     context domain rather than leaving it wiped — a node's own top-level shape not having changed
     means its context is still valid regardless of what its children's own reconstruction did.
  2. Relatedly: several "no rule matched" fallbacks used to call `expr_from_kind(other)` on a
     peeled-off operand — `expr_from_kind` intentionally panics (`unreachable!`) on a leaf
     (`Variable`/`Bool`/`Int`/`Real`), since a leaf's domain can't be recomputed bottom-up from
     children it doesn't have. But a "no rule fired" operand can absolutely *be* a leaf (e.g.
     `f @ X` where `X` is a bare variable and `f` isn't the identity) — every such site now
     reconstructs via `Expr::new(other, saved_domain, saved_context)` using the operand's own
     already-correct fields, captured *before* the match moved its `kind` out (Rust's partial-move
     rules make this fine: moving `e.kind` out via a match scrutinee doesn't prevent using `e`'s
     other fields, like `expression_domain`/`context_domain`, afterward).
- **`normalize_reduction.rs`**: `NormalizeReduction` — extracts every *top-level* `Reduce` in a
  `StandardEquation` into a fresh local + equation (skips `UseEquation`s and nested reductions,
  matching the source exactly). Small and self-contained relative to `normalize.rs`. Note the real
  pipeline order: run this *before* `Normalize` (a `Dependence` directly wrapping a bare `Reduce`
  only reaches normal form once the reduction has somewhere else to live) — see
  `normalize_fixtures.rs` for exactly this sequencing.

## `alpha-codegen`/`alphac` in detail — what exists, what's next

Files: `alpha-codegen/src/{simplec,layout,writec,error}.rs`, `alphac/src/main.rs`.

- **`simplec.rs`**: the `simpleC` model, deliberately minimal (design doc §7: "this was always
  'just a small C AST'") — `Stmt`/`Expr`/`Function`/`CType` as plain enums, with `Expr::Raw(String)`
  as the escape hatch every affine/boolean condition goes through (isl's own C-format printer — see
  `isl::ast`'s doc — already produces valid C text for those, so there's no hand-written
  affine-to-C converter to port at all). Preprocessor lines (`#include`, `#define` macros, global
  var decls) are plain formatted strings, not their own AST nodes, since nothing ever pattern-matches
  on them after they're produced.
- **`layout.rs`**: storage layout for a system's variables. Two schemes, matching what's actually
  observed in the source project's own reference-generated C (read directly as ground truth for
  this port — `~/git/poly/alpha-language/tests/alpha.codegen.tests/resources/{CopyInput,
  PrefixScan,LUDecomposition}/*.c`): **interface variables** (a system's own `inputs`/`outputs`)
  are a plain pointer chain indexed *directly*, no offset, ever — memory for these is the caller's
  responsibility (see the `*-wrapper.c` files: they allocate a full dense block regardless of the
  declared domain's actual shape, e.g. `LUDecomposition`'s triangular `L`). **Flat variables**
  (locals, and every generated flag array) are one allocation this generator sizes/frees itself, via
  a row-major linearization over each dimension's own **bounding box** (`Set::dim_min`/`dim_max`,
  independently per dim) — the isl-only fallback design doc §5 explicitly sanctions in place of the
  source system's exact Barvinok/Ehrhart-derived linearization (visible in its own reference output
  as the piecewise `(i,j) -> rank` formulas on every flag array in `LUDecomposition.c`) — implementing
  that exactly is real, separate work still gated behind the (currently stub) `barvinok` feature.
  For a rectangular domain the two coincide exactly; for a triangular one this allocates strictly
  more than the source system does, never less, so it stays correct.
- **`writec.rs`**: the `WriteC` generator itself. Every equation becomes a memoized
  `eval_<Var>(indices...)` function guarded by the same `'N'`/`'I'`/`'F'` flag convention as the
  source system; every `Reduce` becomes its own synthesized `reduce<N>(params..., ambient_p...)`
  function (ambient index values passed in "primed" — `ip`, `jp`, ... — matching the source
  system's own naming); loops (the driver's "evaluate all outputs" nest, and each reduce's own
  summation loop) are generated via `isl_ast_build` over an identity schedule, never hand-rolled;
  `case` branches become a right-nested ternary, with the *last* branch always unconditional (sound
  because phase 6's completeness check already guarantees the branches jointly cover the domain) and
  every other branch's guard rendered from its own `context_domain` via `AstBuild::expr_from_set` —
  deliberately *not* from inspecting whether it's wrapped in an explicit `Restrict`, since
  `context_domain` is already the uniformly-correct "when is this branch live" answer regardless of
  a branch's particular shape post-`Normalize`. `Restrict`/`AutoRestrict` are always transparently
  peeled during *value* generation (they only ever contribute a domain, consumed elsewhere — a case
  guard, or a reduce's loop bounds — never the computed value itself).
  **Scope, relative to the source system's own `WriteC`** (every boundary raises a named
  `CodegenError::Unsupported`, never a panic or a bare isl error — see `codegen_fixtures.rs`'s
  `is_known_scope_boundary` for the exact list): `UseEquation` (no codegen backend, matching the
  source system exactly — not a port regression); `Convolution` (unreachable in practice — already
  excluded at lowering, per `domain.rs`'s own gap); `Select`/`IndexPolynomial` (`val{...}`)/
  `argreduce` (real features, genuinely not implemented — the whole 82-fixture corpus has zero
  `argreduce` uses and only a handful of `select`/`val{...}` uses); a `case`'s `auto` branch whose
  true domain isn't independently bounded once combined with only the enclosing reduce/equation's
  own ambient context (see bug #14 below — a real, rare tree shape, not a crash to hide).
- **`alphac/src/main.rs`**: the CLI driver, `alphac file.alpha [-o file.c]`. Per system found in the
  file (walking nested `AlphaPackage`s): `alpha_model::analyze_system` (the new consolidated
  entry point, see below) → if clean, `lower → NormalizeReduction → Normalize(deep=true) →
  alpha_codegen::generate_system`. A file with more than one system gets one self-contained
  generated-C block per system, concatenated (each carries its own `#include`/macro preamble).
- **`alpha_model::analyze_system`/`analyze_root`** (new this session, in `alpha-model/src/analyze.rs`):
  the consolidated "run all of alpha-model" entry point `alphac` (and any future driver) needed —
  previously every phase's own fixture test wired phases 1–6 together itself (e.g.
  `completeness_fixtures.rs`'s `check_all`); this is that wiring, promoted to a real `pub` function
  and reused by `alphac` directly instead of a fourth copy.
- **Validation**: `codegen_fixtures.rs` runs the whole pipeline over all 82 fixtures — 199 systems
  generate successfully, 21 hit a named scope boundary, zero unexpected failures. Separately (no
  automated test for this yet — see "immediate next steps"), generated C for the three real
  `alpha.codegen.tests` reference programs was compiled and linked against the sibling Java
  system's own `*-wrapper.c`/`*_verify.c` files and **passes real numeric verification**
  (`TEST for Y/L/U PASSED`) across `N` = 1, 2, 5, 20, 30, 50 — this is the strongest evidence the
  generator is actually correct, not just "produces syntactically valid C".

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

And in `alpha-model`'s `domain.rs` (phases 3–4, this session):

9. `Set::universe`/`Set::empty` (in `isl`) used to construct their `Set` unconditionally without
   checking isl's null-on-error convention (`debug_assert!(!ptr.is_null())` inside `from_raw`
   would panic instead of returning a `Result`) — the one place in the `isl` crate that didn't
   follow its own stated "every fallible call returns `Result`" rule. Found via an actual panic:
   `isl_set_universe` on a `PwQPolynomial`'s raw `.space()` fails an internal isl assertion
   (`space->n_in == 0`) because that space is map-shaped (`[in]->[out]`), not set-shaped — fixed
   by (a) making both functions fallible, and (b) adding `domain_space()` accessors to
   `MultiAff`/`PwQPolynomial` (bound to `isl_{multi_aff,pw_qpolynomial}_get_domain_space`) so
   callers get the actual set-shaped domain space instead of the raw map-shaped one.
10. `StandardEquation`'s own context domain must combine the variable's declared domain with the
    enclosing `SystemBody`'s (0-dimensional) parameter domain via `intersect_params`, not
    `intersect` — they have different dimensionalities, and plain `intersect` fails with "spaces
    don't match" (mirrors the source Java's `variable.domain.intersectParams(systemBody.parameterDomain)`,
    easy to misread as a plain intersect at a glance).
11. `RestrictExpression`/`SelectExpression` REPLACE the ambient index-name context (not extend —
    see the `domain.rs` entry above); getting this backwards doesn't error on `eval_function` calls
    (a too-large context still parses as a valid, just wrong, function) but does produce a
    dimension mismatch downstream when intersecting against the restrict/select's own domain.

And in `alpha-codegen`/`alpha-transform` (`writec.rs`/`normalize_reduction.rs`, this session):

12. `isl_ast_build_expr_from_set(build, set)` renders using **the build's own context set's dim
    names**, positionally — the `set` argument's *own* dim names are ignored entirely, even if
    completely different or anonymous. Verified empirically (a throwaway isl-sys example) before
    relying on it: a build from `[N]->{[i,j]:...}` correctly renders a condition set literally
    named `[N]->{[x,y]:x=0}` as `i == 0`. This is what makes `writec.rs`'s approach work at all —
    `alpha_model::domain`'s own real `Set`s frequently carry anonymous or synthetic dim names (see
    #13), so codegen builds its *own* small "universe" context set with exactly the C identifiers
    it wants, once per equation, and never worries about aligning names on the sets it's rendering.
13. Relatedly: a `Set`'s own dim names, as constructed by `alpha_model::domain`'s resolver
    pipeline, are **not reliably the equation's real source names** — an unnamed `RectangularDomain`
    (e.g. `outputs Y:[N]`, no explicit `{[i]:...}`) gets an internal synthetic name
    (`__rect0`), and plenty of intermediate `Set`s carry no name at all. `alpha_transform::ir`
    grew two narrow fields purely to carry the *real* names through as a naming hint for codegen
    (`StandardEquation::index_names`, `ExprKind::Reduce::body_context`) — `Normalize` itself never
    reads them (isl set/map algebra never needed dimension *names*, only *counts*, which is why
    `Normalize` got away without tracking any of this). `writec.rs`'s `pick_names` prefers the hint
    but always falls back to synthesizing `i0,i1,...`/`k0,k1,...` if it's missing, wrong-length, or
    not a valid/safe C identifier — codegen is correct either way (every isl object it prints from
    is explicitly renamed to match via `MultiAff::set_dim_name` first), so this is purely cosmetic.
14. A `Reduce`'s own `expression_domain` is a *purely bottom-up* derivation (see `normalize.rs`'s
    `expr_from_kind`) — it has no reason to be bounded in the *ambient* dims. `PrefixScan`'s
    `reduce(+, [j], {:j<=i} : X[j])` only ever bounds `j` relative to `i`, never bounds `i` itself.
    Using it alone as the reduce's iteration domain made `Set::dim_max`/`dim_min` fail with isl's
    "unbounded optimum" the moment ambient dims got moved to params — found by an actual `alphac`
    run against `PrefixScan.alpha`, not by inspection. Fixed by intersecting with `context_domain`
    (the complementary top-down piece that *does* carry the ambient equation's own bounds) before
    computing anything. A related, rarer case survives even after that fix: `Normalize`'s own "D :
    auto : E -> auto : E" rule (`normalize.rs`) *discards* a wrapping restrict for an `auto` branch
    specifically (by design — an `auto` branch's true domain is meant to come from "parent context
    minus siblings", not from a restrict that happened to wrap it) — if a reduce's own explicit
    restrict was the *only* source of ambient bounds (e.g. a 0-dimensional output whose whole body
    is one `reduce(..., case {..; auto: ...})`), the `auto` branch's resulting domain can end up
    genuinely unbounded. `writec.rs`'s `isl_bound_err` classifies isl's "unbounded" failures from
    `dim_max`/`dim_min`/`AstBuild::generate` as a named, documented scope boundary rather than
    letting a raw isl error (or worse, a wrong answer) surface — a real limitation of this session's
    from-context bound derivation, not a crash to paper over.
15. `NormalizeReduction`'s extracted local (the `_NR` variable) needs its own domain **intersected
    with the enclosing equation's target-variable domain**, not just the extracted `Reduce`
    node's own (bottom-up, per #14) `expression_domain` — otherwise the same "unbounded in the
    ambient dims" problem infects the new local's own storage sizing. Fixed in
    `normalize_reduction.rs::apply` by looking up every variable's domain once, up front, before
    the mutating pass, and threading the enclosing equation's own domain down into
    `extract_top_level` for exactly this intersection.

## Immediate next steps (in order)

1. VS Code extension (napi-rs native addon + TextMate grammar — design doc §8) — the one piece of
   the original phased roadmap (design doc §9) with nothing built yet.
2. A dedicated `alphac`-level test (compile the generated C, or at least parse it with a real C
   frontend) — today the three reference fixtures were verified *manually* this session (compiled
   and linked against the sibling Java system's own `*-wrapper.c`/`*_verify.c`, run across several
   `N`, `TEST for Y/L/U PASSED`); nothing in the test suite exercises this automatically yet, so a
   regression here wouldn't be caught by `cargo test`. Needs a C compiler on the test machine
   (`cc`/`clang`), which isl/isl-sys don't currently require — worth deciding whether that's an
   acceptable new test-time dependency before wiring it up.
3. `alpha-codegen`'s own documented scope boundaries, if a real program ever needs them: `Select`
   (relation-based reindexing — only 3 fixtures use it), `IndexPolynomial`/`val{...}` (needs
   `PwQPolynomial` C-format printing, unverified whether isl even supports it — `Set`'s own C-format
   printing does *not*, discovered this session by an actual failed printer call, not assumed),
   `argreduce` (zero fixtures use it). None of the 82 real fixtures currently require any of these
   beyond what's already covered.

Lower priority, not blocking anything above: the three documented scope boundaries in
`alpha-model` (convolution's own domain in `domain.rs`; `UseEquation`'s context domain, which
would also make `completeness.rs`'s `check_use_equation_outputs` live; `check_use_equation_recursion`'s
bare-name self-recursion check) could be revisited if a real program ever needs them — none of the
82 real fixtures currently require it beyond what's already covered. Note `alpha-transform`
inherits both of the `alpha-model` gaps automatically (`lower.rs` skips equations either one would
affect), so closing them in `alpha-model` is what unblocks `alpha-transform`/`alpha-codegen` for
that remaining handful of equations too — no separate `alpha-transform`-side work needed. Also
note `UseEquation`/subsystem calls have no codegen backend in the source system's own `WriteC`
either (design doc §7) — `alpha-codegen` matching that isn't a port regression, and closing the
`domain.rs`/`completeness.rs` gaps above wouldn't change that; a working `UseEquation` codegen
backend would be new work beyond what the source system itself does.

## Where to look for more context

- `docs/rust-port-design.md` — the full design doc (scope, naming conventions, crate layout,
  parsing strategy, ISL binding strategy + licensing, codegen plan, VS Code architecture,
  phased roadmap). Read this for *why*, not just *what*.
- `~/.claude/projects/-Users-anna-git-poly/memory/` — cross-session memory (currently just one
  entry: don't run `git add`/`commit`/`status` proactively in this repo).
