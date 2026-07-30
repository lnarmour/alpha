# Alpha Language: Rust Port — Design & Migration Plan

Status: draft for review. No code written yet.

## 0. Scope, as agreed

- **Port target**: `alpha-language` core only — parse → semantic analysis → AST → C codegen
  (the demand-driven `WriteC` path). The `alphaz`/GeCoS scheduling search, tiling, memory-mapping,
  and reduction-simplification machinery is explicitly **out of scope** for this phase.
- **Polyhedral engine**: bind to the existing C **isl** library via Rust FFI, not a pure-Rust
  reimplementation. isl's own textual parser, set/map algebra, and AST builder are what the
  current system actually depends on for correctness — there is no upside to re-deriving that
  algorithmically in Rust, and every serious polyhedral tool (Polly/LLVM, PPCG, Pluto's ISL mode)
  makes the same call.
- **No Xtext/ANTLR/EMF/OSGi anywhere.** The grammar, AST, validators, and code generator are
  hand-ported to native Rust crates using ordinary Rust parsing tooling (below).
- **GeCoS/ISL bindings live in their own crate family**, decoupled from the Alpha compiler itself,
  since they're a reusable "bind libisl/barvinok from Rust" concern, not Alpha-specific logic.
- **IDE target**: VS Code, with a tighter (non-LSP) integration — the Rust core is exposed to the
  extension as an in-process native addon rather than a generic language-server-protocol process.

## 1. What we're actually porting (condensed survey)

Full detail is in the research pass this doc is based on; the load-bearing facts:

- **Grammar** (`bundles/alpha.model.xtext/src/alpha/Alpha.xtext`, 624 lines): systems
  (`affine Name [params] -> {domain} inputs/outputs/locals ... let ...`), piecewise system
  bodies (`when`/`else`), equations (`StandardEquation`, `UseEquation` for subsystem calls),
  a fairly rich expression grammar (`case`, `reduce`/`argreduce`, `conv`, `select`, dependence
  access `X[expr]`/`f@expr`, restrict `{dom}: expr`, `auto:`), and a small "calculator" algebra
  over domains/relations/functions (`define X = <calc expr>`).
- **Crucial wrinkle**: the text inside `{ ... }` domain/relation/function literals is *not*
  parsed by the grammar at all — it's captured as an opaque token-soup string and handed to
  **isl's own string parser** later, during semantic analysis. So "the domain sub-grammar" is
  really isl's Presburger-arithmetic syntax, not something Alpha itself defines. A Rust port
  should treat this the same way: capture the substring, hand it to `isl_set_read_from_str`
  (etc.) via the FFI crate, and turn isl parse errors into diagnostics.
- **AST** (`bundles/alpha.model/model/alpha.xcore`, 1433 lines): `AlphaRoot` →
  `AlphaSystem` → `SystemBody` → `Equation` (`StandardEquation` | `UseEquation`) →
  `AlphaExpression` tree. Every expression node carries two *computed* ISL sets (expression
  domain, context domain) as semantic-analysis output, not syntax. There's also a separate,
  smaller "calculator" AST (`JNIDomain`/`JNIRelation`/`JNIFunction`/...) with its own dynamic
  type tag (`SET`/`MAP`/`FUNCTION`/`POLYNOMIAL`).
- **"Type checking"**: there's no scalar type system. What exists is (a) resolving calculator
  expressions into typed ISL objects and checking operator/type compatibility, and (b) a fixed
  six-phase pipeline over the whole `AlphaRoot`:
  1. `JNIDomainCalculator` (interface pass) — resolve system/variable domains.
  2. `JNIDomainCalculator` (expression pass) — resolve calculator exprs inside bodies.
  3. `ExpressionDomainCalculator` — bottom-up expression-domain inference.
  4. `ContextDomainCalculator` — top-down context-domain inference.
  5. `AlphaNameUniquenessChecker` — duplicate name checks.
  6. `UniquenessAndCompletenessCheck` — the real "type errors": incomplete/overlapping system
     bodies, incomplete/overlapping equations, undefined variables, unbounded reductions, etc.
     (~30 distinct diagnostics, cataloged in `AlphaIssueFactory`).
- **Codegen** (`bundles/alpha.codegen`): Alpha AST → `Normalize`/`NormalizeReduction` (push
  dependences to leaves) → a small "simpleC" AST (`simpleC.xcore`, deliberately minimal C
  subset) → printed C text. The demand-driven `WriteC` generator has no real scheduling: it
  compiles every equation to a memoized `eval<Var>(indices...)` function with a `NOT_EVALUATED
  /IN_PROGRESS/EVALUATED` flag array, and generates loops via `isl_ast_build` over the identity
  schedule. `ReduceExpression`s get synthesized into standalone `reduce<N>()` functions built
  from direct ISL constraint construction. **Known gap in the existing system, not just ours**:
  `WriteC` throws on `UseEquation` — subsystem calls have no working codegen today.
- **ISL surface actually used by core**: string parsing (set/map/aff/pwqpolynomial); boolean
  set/map algebra (intersect/union/subtract/complement/hulls); apply/preimage; dimension/space
  manipulation; constraint construction; `isl_ast_build` for loop generation; `isl`'s own
  C-format pretty-printer for affine/polynomial/constraint expressions; optionally Barvinok's
  cardinality counting for `malloc` sizing. This is a real subset of full isl — see §5.

## 2. Naming conventions for the port

The source project is Eclipse/Java-first, and a lot of its names reflect that heritage rather
than the language's own semantics. Rule of thumb: **strip Eclipse/Java/JNI artifacts; keep
Alpha's and the polyhedral-model's own vocabulary verbatim.**

- **Drop entirely**: the `JNI*` prefix (`JNIDomain` → `Domain`, `JNIRelation` → `Relation`,
  `JNIFunction` → `Function`, `JNIPolynomial` → `Polynomial`, `JNIDomainInArrayNotation` →
  `ArrayDomain`, `JNIFunctionInArrayNotation` → `ArrayFunction`, `JNIParamDomain` →
  `ParamDomain`). These prefixes only ever meant "backed by the JNI isl binding" — irrelevant
  once ISL access is an ordinary Rust FFI call. Similarly, class-name suffixes that exist only
  because of Xtext/EMF conventions (`*InternalStateConstructor`, `*Impl`, generated `*Factory`
  boilerplate) don't need Rust analogues at all — e.g. `AlphaInternalStateConstructor`'s pipeline
  becomes a plain `pub fn analyze(root: &ast::Root) -> SemanticModel` in `alpha-model`, no
  "constructor object" needed.
- **Drop the redundant `Alpha` stutter on AST node names.** Inside `alpha_syntax::ast`, a node
  called `AlphaExpression` is redundant with its own module path — Rust convention (and
  rust-analyzer's own `syntax::ast`, which this design otherwise follows) is `ast::Expr`,
  `ast::System`, `ast::Root`, `ast::CaseExpr`, etc., not `ast::AlphaExpression`/`ast::AlphaRoot`.
  Open to reverting this per node if a shorter name would collide or genuinely lose clarity —
  flag it if you'd rather keep `Alpha`-prefixed names for searchability against the Java source.
- **Preserve verbatim — these are Alpha/polyhedral-model terms, not Eclipse artifacts**:
  "expression domain" and "context domain" (the two per-node inferred ISL sets — keep exactly
  this terminology in field names, e.g. `Expr::expression_domain()`/`Expr::context_domain()`,
  since anyone who knows the Alpha literature will look for these terms specifically), "system",
  "system body", "equation", "reduce"/"argreduce", "restrict", "case", "select", "convolution",
  "fuzzy variable", "calculator expression", "gist" (isl's own term, keep as-is), "normalize"/
  "normal form" (the `Normalize` transformation pass).
- ISL types themselves keep isl's own naming (`Set`, `Map`, `Aff`, `MultiAff`, `Space`,
  `Constraint`, ...) inside the `isl` crate, just without the redundant `ISL`/`isl_` prefix the
  C API and the JNI wrapper both need but a Rust module namespace (`isl::Set` vs. `isl_set`)
  doesn't.
- If a rename during implementation turns out ambiguous or contested, ask rather than guessing —
  this section captures intent, not an exhaustive lookup table.

## 3. Workspace layout

Two independent crate families in one Cargo workspace (or two workspaces if you want the ISL
bindings publishable/reusable outside this project from day one — recommend starting as one
workspace, splitting later if it proves useful independently):

```
alpha-lang/                      (workspace root)
├── isl-sys/                     raw FFI bindings (bindgen) to libisl
├── isl/                         safe, idiomatic Rust wrapper over isl-sys
├── barvinok-sys/                raw FFI bindings to libbarvinok  (separate crate: GPL, see §5)
├── barvinok/                    safe wrapper, feature-gated, optional dependency everywhere
├── alpha-syntax/                lexer + lossless CST + parser + typed AST view
├── alpha-model/                 semantic model: name resolution, the 6-phase checker, diagnostics
├── alpha-transform/             Normalize, NormalizeReduction (only transforms codegen needs)
├── alpha-codegen/                simpleC model + WriteC demand-driven generator + isl-AST bridge
├── alphac/                      CLI binary: alphac file.alpha -o file.c  (the "loader" + driver)
└── vscode/                      the VS Code extension (TypeScript + native addon, see §8)
```

Why split `isl`/`barvinok` out from the Alpha-specific crates: this is exactly your instinct
about "separate rust bindings/crate for the gecos tools" — the isl/barvinok FFI layer is a
general-purpose "bind a polyhedral C library from Rust" concern with its own release cadence,
its own testing needs (round-trip every isl operation independently of Alpha), and its own
licensing profile (see §5). If a future `alphaz`-equivalent port ever needs more of the GeCoS
tool family (graph tools, TOM mapping), those become sibling crates (`gecos-graph-sys`, etc.)
next to `isl-sys`/`barvinok-sys`, not bolted onto the Alpha compiler.

## 4. Parsing & the syntax layer — no Xtext, ordinary Rust tooling

Recommendation: **`logos` for lexing + a hand-written recursive-descent/Pratt parser + `rowan`
for the tree representation**, i.e. the same architecture rust-analyzer, rust's own `rustc_parse`
lineage, and most modern "IDE-first" language tools use. Concretely:

- **`logos`**: derive-macro lexer, fast, trivial to write and to extend when you add tokens for
  the calculator-language keywords (`domain`, `range`, `cross`, etc.).
- **Hand-written recursive-descent parser** for the statement/declaration grammar, with a
  **Pratt parser** for the expression precedence chain (the grammar's own precedence chain —
  `if → restrict → or → and → relational → additive → multiplicative → minmax → unary` — maps
  directly onto Pratt binding powers). Hand-written beats a parser-generator (`pest`/`lalrpop`)
  here because of two Alpha-specific irregularities that generators handle poorly: (a) the
  opaque "capture until matching brace" domain/relation/function literals, and (b) the
  `over`/`with` optional-clause soup in `UseEquation`. A hand-written parser can special-case
  brace-matching for `{...}` capture directly, generators fight you on it.
- **`rowan`** for the actual tree data structure: a **lossless, error-tolerant concrete syntax
  tree** (every token, whitespace, and comment preserved; parse errors don't abort the parse,
  they get attached to a partial tree and the parser recovers at the next statement boundary).
  This is the single most important choice for the "parsing errors" half of your ask: with a
  resilient CST, the editor still gets a usable tree (hover, outline, partial diagnostics) while
  the user is mid-edit with invalid syntax — a plain `Result<Ast, Error>` recursive-descent parser
  that bails on the first error would make the editing experience much worse. On top of the
  rowan `SyntaxNode`, add a thin typed `ast::` layer (typed accessor structs wrapping
  `SyntaxNode`, exactly rust-analyzer's `syntax::ast` pattern) so semantic analysis and codegen
  work with ergonomic typed nodes instead of raw green/red tree traversal.
- Diagnostics from this layer: unclosed braces, unexpected tokens, malformed literals — plus,
  crucially, isl's *own* parse errors on domain/relation/function text get surfaced through the
  same diagnostic sink once semantic analysis calls into `isl-sys`.

This gives you syntax highlighting for free two ways: (a) a static TextMate grammar for the
VS Code editor's first-paint tokenization (cheap, no Rust involved, instant highlighting even
before the extension loads), and (b) a semantic-tokens pass computed from the real rowan tree
for accurate, context-aware highlighting (e.g. distinguishing a `Variable` reference from an
`AlphaSystem` name, which a regex-based TextMate grammar structurally cannot do).

## 5. The ISL / Barvinok bindings crate — and a licensing decision you need to make

**isl is MIT.** **Barvinok is GPL**, and (since barvinok 0.30) barvinok vendors isl internally,
but the standalone `isl` project itself remains MIT — you can and should depend on isl alone
without pulling in barvinok's GPL surface.

This matters because Barvinok is only used for one thing in the core compiler: cardinality
(Ehrhart polynomial) counting, for `malloc` sizing and `val[...]` polynomial-index expressions
(§4 of the survey, `WriteCExprConverter`/`getCardinalityExpr`). Recommendation:

- Put Barvinok bindings in their **own crate** (`barvinok-sys`/`barvinok`), separate from `isl-sys`/`isl`,
  as already reflected in §3's layout — this isn't just cleanliness, it's a license boundary.
  Anything that links `barvinok` transitively becomes subject to GPL obligations on distribution;
  anything that only links `isl` does not.
- Feature-gate cardinality counting in `alpha-codegen` behind a `barvinok` Cargo feature, off by
  default. Ship `alphac` (the CLI) and, especially, the **VS Code native addon** built *without*
  the `barvinok` feature — the IDE-facing parts (parsing, diagnostics, hover) never need
  cardinality counting at all, so there's no reason to drag a GPL C library into a shipped
  VS Code extension binary. A separate `alphac --features barvinok` build (or a distinct binary)
  can be used for actual C-file generation where malloc-sizing is needed, distributed under
  terms that account for the GPL dependency (**Resolved: yes** — feature-gate it, ship `alphac` and the VS Code addon MIT-only by default
  (see §10).
- If you'd rather sidestep this entirely: cardinality counting for `malloc` sizing can often be
  computed directly from isl (MIT-only) for the box/rectangular-domain cases that cover the vast
  majority of real programs, falling back to a runtime-computed size (loop-and-count, or a
  conservative overallocation) instead of a compile-time Ehrhart polynomial when the domain isn't
  simple. Worth prototyping before committing to a Barvinok dependency at all.

**Binding approach**: `bindgen` against the system-installed `libisl` headers for `isl-sys`
(you already have the isl C source at `~/git/isl` for reference/local building), with a
`build.rs` that first tries `pkg-config`, then falls back to building a vendored copy from
source (isl depends on GMP, which complicates static-linking on Windows — same constraint the
current Eclipse-based system already has, so no regression there; Windows can stay
WSL2-or-unsupported same as today per the existing install docs).

**Safe wrapper (`isl` crate) surface**, derived directly from the operation inventory in the
survey — this is a *bounded*, well-defined API, not "bind all of isl":
- Types: `Set`, `BasicSet`, `Map`, `BasicMap`, `Aff`, `MultiAff`, `AffList`, `PwQPolynomial`,
  `Space`, `DimType`, `Constraint`, `Context`, `UnionMap`, `UnionSet`.
- Ops: string parsing (`Set::read_from_str` etc.), `intersect`/`union`/`subtract`/`complement`,
  `apply`/`preimage`, hulls (`affine_hull`/`polyhedral_hull`/`convex_hull`), `is_equal`/
  `is_empty`/`is_disjoint`, dimension manipulation (`project_out`/`move_dims`/`add_dims`/
  `set_dim_name`), `gist`, constraint building (`Constraint::equality`/`set_coefficient`),
  `to_string` in both ISL and **C** format (isl's C pretty-printer is load-bearing for codegen —
  don't reimplement an affine-expression-to-C printer, keep using isl's).
- AST builder: `AstBuild::from_context(...).set_iterators(...).generate(...)` plus the node kinds
  (`AstNode::{For, If, Block, User}`) — this is the single highest-value piece of isl to bind
  well, since it's where isl's actual loop-generation algorithm earns its keep.
- Every isl call that can fail (malformed input, isl's own internal errors) returns
  `Result<T, IslError>`, mirroring the existing Java codebase's `callISLwithErrorHandling`
  pattern — worth keeping, since it's exactly the pattern needed to turn native isl errors into
  Alpha diagnostics instead of process aborts/panics.
- One correctness note from the Java code worth carrying forward deliberately: ISL objects are
  consumption-oriented (many isl C API calls take ownership of / free their inputs). The JNI
  layer's pervasive `.copy()` defensive-copying is compensating for that. In Rust this is a much
  better fit for the type system than it was for Java: model ownership-consuming isl calls as
  taking `self` by value (not `&self`), so the borrow checker enforces "you can't reuse an isl
  object after an operation that consumes it" at compile time, and `Clone` (backed by isl's real
  `_copy` functions) becomes the only way to reuse a value across two operations. This eliminates
  a whole class of bugs the original defensive-copying was working around by hand.

## 6. Semantic model / typechecking port plan

Port `AlphaInternalStateConstructor`'s six phases as six ordinary Rust passes over the typed AST
(from `alpha-syntax`) plus a resolved semantic model built alongside it, living in `alpha-model`:

```rust
// sketch, not final API
pub struct SemanticModel {
    systems: HashMap<SystemId, System>,
    diagnostics: Vec<Diagnostic>,
}

pub fn analyze(root: &ast::AlphaRoot) -> SemanticModel { /* the 6 phases, in order */ }
```

- Phases 1–2 (`JNIDomainCalculator` interface + expression passes) → resolve every domain/
  relation/function text fragment via the `isl` crate's string parser, threading the "in-scope
  index names" context exactly as the Java version's `Stack<List<String>>` does (a plain
  `Vec<Vec<String>>`/scope-stack in Rust). Also ports: sibling-domain inheritance in comma-lists,
  implicit "else" system-body domain completion, `UseEquation` arity checks.
- Phase 3 (`ExpressionDomainCalculator`) / Phase 4 (`ContextDomainCalculator`) → straightforward
  recursive functions over the typed AST (no visitor-interface indirection needed in Rust —
  exhaustive `match` on the expression enum replaces the Java visitor pattern the original uses
  purely for double-dispatch; this is one of the places the Rust port gets simpler than the
  source, not just equivalent).
- Phase 5 (`AlphaNameUniquenessChecker`) → straightforward.
- Phase 6 (`UniquenessAndCompletenessCheck`) → the real "type errors": port the ~30 diagnostics
  from `AlphaIssueFactory` as a Rust `enum Diagnostic { IncompleteEquation { .. }, ... }` with a
  `Display`/rendering impl, each variant carrying enough structured data (offending domain as a
  gisted isl set, source span from the rowan tree) to render both a human message and a
  machine-readable code for the editor.
- The **calculator type system** (`CalculatorExpressionEvaluator`'s dispatch-over-dynamic-type
  table, SET/MAP/FUNCTION/POLYNOMIAL × unary/binary op) ports directly to a Rust `match` over
  `(Op, ValueKind, ValueKind)` returning `Result<Value, Diagnostic>` — this is explicitly a
  finite, enumerable compatibility matrix, a natural fit for exhaustive matching.
- One deliberate simplification vs. the source: fix the "named constants via regex string
  substitution before handing text to isl" hack (`AlphaUtil.replaceAlphaConstants`) by resolving
  `AlphaConstant` references during lexing/parsing instead — the lexer/parser already knows where
  identifiers are, so substituting textually before constructing the isl-bound string is strictly
  safer than post-hoc `replaceAll`.

## 7. Codegen port plan

**Fidelity target: semantic equivalence, not bit-for-bit output matching.** The generated C needs
to compute the same result with the same "spirit" (same overall strategy: memoized demand-driven
evaluation, same complexity characteristics) — it does not need to reproduce the source project's
exact variable naming, statement ordering, or formatting choices. Take liberties wherever the
Rust-idiomatic or simply cleaner choice diverges from the Java source; don't carry forward a
workaround or awkward construction just to match output byte-for-byte. (This also loosens the
`NameChecker`/`ProgramPrinter` port specifically — replicate their *behavior* — no collisions,
valid C, readable output — not their exact formatting rules.)

Straightforward, mechanical port of §4 of the survey:
- `alpha-transform`: `Normalize` + `NormalizeReduction` only (the rest of the transformation
  family is scheduling-adjacent and deferred per scope).
- `alpha-codegen`: the `simpleC` model as plain Rust enums/structs (no Xcore/EMF needed — this
  was always "just a small C AST", nothing about it needs a modeling framework); `ASTConverter`/
  `LoopGenerator`/`AffineConverter`/`ConditionalConverter`/`PolynomialConverter` port near
  1:1 against the `isl` crate's AST-builder API from §5; `WriteC`'s memoized `eval<Var>` +
  flag-array cycle detection, and the per-`ReduceExpression` synthesized `reduce<N>()` function
  generation (including the direct ISL-constraint construction in `createReduceLoopDomain`) port
  close to line-for-line, since that logic is really "isl operations in a particular sequence,"
  not Java-specific.
- Known, inherited limitation to carry forward explicitly (not a new gap introduced by the
  port): `UseEquation`/subsystem calls have no codegen backend in `WriteC` today either — match
  that (clear error diagnostic on attempting to generate C for a system containing a
  `UseEquation`), don't silently under-scope it as if it were a port regression.
- `alphac`: the CLI driver — `parse → analyze → Normalize/NormalizeReduction → generate → print`,
  replacing `alpha.loader`'s role, deliberately *not* coupled to any schedule-tree grammar (the
  survey flags the current `alpha.loader`'s coupling to `alpha.targetmapping` as an accidental
  artifact of shared Guice injector setup, not a real dependency — the Rust port shouldn't
  reintroduce it).

## 8. VS Code extension architecture

Per your call to skip a generic LSP server in favor of tighter, VS Code-specific integration:

- **Native addon via `napi-rs`**: `alpha-syntax` + `alpha-model` compiled into a `cdylib` loaded
  in-process by the extension host through N-API bindings (no subprocess, no JSON-RPC framing
  overhead, direct function calls from TypeScript into Rust: `parse(text) -> Diagnostics`,
  `hoverInfo(text, offset) -> ...`, etc.). This is what "tighter" buys you over LSP: you can
  expose exactly the calls VS Code's extension API wants (custom decorations, code actions,
  bespoke tree views for the AST/domains) without shoehorning them through LSP's message shapes,
  and there's no separate server process to manage/restart/version-skew against the client.
- **Prebuilt binaries**: `napi-rs`'s standard cross-compilation story (GitHub Actions matrix:
  macOS arm64/x64, Linux x64/arm64; Windows only if/when you decide to support it, matching the
  existing project's Linux/macOS-only stance) — ship one `.node` file per platform, npm package
  picks the right one at install time. This crate is built **without** the `barvinok` feature
  (see §5) — the extension only ever needs parse/diagnostics/hover, never C-generation.
- **Syntax highlighting**: static TextMate grammar (JSON, hand-written from the token/keyword
  inventory in §1/§4) for immediate first-paint highlighting, optionally upgraded later to a
  semantic-tokens provider backed by the real rowan tree for accuracy LSP/TextMate regexes can't
  match (e.g., correctly coloring a `Variable` reference vs. an `AlphaSystem` name, which are
  lexically identical).
- **Diagnostics**: on every document change (debounced), call into the native addon's
  `parse+analyze` entry point, map the resulting `Diagnostic` structs (source spans already in
  UTF-8 byte or line/col form from rowan) directly to VS Code's `Diagnostic`/`DiagnosticCollection`
  API.
- Because there's no LSP server, none of this transfers to other editors for free — that's the
  explicit tradeoff of "VS Code-only, tighter integration" over "LSP server + thin client," and
  matches what you chose. If you ever want Neovim/Zed/Helix support later, the natural path is
  to additionally stand up a thin `tower-lsp` wrapper around the *same* `alpha-syntax`/
  `alpha-model` crates — nothing above precludes that later, it's just not built now.

## 9. Phased roadmap

1. **Grammar + lossless parser**: `logos` lexer, hand-written parser, rowan CST, typed `ast::`
   layer. Conformance target: parse every fixture under `tests/alpha.model.tests/resources/
   src-valid/**` without error, and every fixture under `src-invalid/syntax-tests/**` with the
   right recovered error.
2. **`isl-sys`/`isl` crate**, built and tested standalone against the bounded operation set in
   §5, independent of Alpha — this can genuinely proceed in parallel with (1).
3. **Semantic model / six-phase checker** (`alpha-model`), targeting the same fixture set,
   including the `src-invalid/undefined-locals-outputs` and `unbounded-reduction` negative
   fixtures as direct regression tests for the diagnostic catalog.
4. **`Normalize`/`NormalizeReduction` + `WriteC` demand-driven codegen** (`alpha-transform`,
   `alpha-codegen`), targeting `tests/alpha.codegen.tests/resources/{PrefixScan,LUDecomposition,
   CopyInput,WriteCTest}` — compare generated C output (or its compiled behavior) against the
   existing Java system's output on the same fixtures as the acceptance bar.
5. **`alphac` CLI**, wiring 1–4 together end-to-end.
6. **VS Code extension**: TextMate grammar first (immediate value, no Rust needed), then the
   napi-rs addon for live parse/diagnostics, then hover/semantic tokens.

## 10. Resolved decisions & remaining open questions

Resolved:
- **Barvinok**: feature-gated behind `barvinok`, its own crate (`barvinok-sys`/`barvinok`), off by
  default. `alphac`'s default build and the VS Code native addon ship MIT-only, no GPL surface.
  Cardinality/`malloc`-sizing is unavailable unless a consumer explicitly opts into the
  `barvinok` feature.
- **Codegen fidelity**: semantic equivalence, not bit-for-bit output matching (§7). Take
  Rust-idiomatic liberties; don't preserve Java-shaped workarounds for their own sake.
- **Diagnostics**: closed enum (§6), matching the fixed catalog the source project already
  established.
- **Naming**: strip Eclipse/Java/JNI-specific artifacts; preserve Alpha/polyhedral-model
  terminology verbatim (§2).

Still open / to raise as they come up during implementation, per your steer to ask rather than
guess on anything ambiguous:
- Any AST node rename in §2 that turns out ambiguous, or where dropping the `Alpha` prefix would
  cause a real naming collision.
- Whether the isl-only cardinality fallback (§5) is good enough in practice, once there's a
  corpus of real fixtures to test it against — may still end up needing the `barvinok` feature
  for non-trivial domains.

Sources consulted for the licensing point in §5: [islpy on Libraries.io](https://libraries.io/pypi/islpy).
