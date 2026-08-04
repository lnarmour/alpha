# Scheduled codegen: design spec (v1)

Status: **draft, for iteration**. Not yet implemented. Branch: `louis/scheduled-codegen`.

## 1. Goal

`alpha-codegen` currently has one backend, `WriteC` (`writec.rs`): a demand-driven, memoized
generator — every output/local variable becomes a recursive `eval_<Var>(idx...)` function guarded
by a `NOT_EVALUATED`/`IN_PROGRESS`/`EVALUATED` flag array. Loop *order* is never actually chosen by
the compiler; it falls out of whatever order the recursive calls happen to unwind in, with the
top-level driver loop just walking outputs in plain lexicographic order (an identity schedule fed
to `isl_ast_build`).

This adds a second backend — call it **`ScheduledC`** — that takes an explicit, user-supplied
**target mapping** (the schedule) and emits a single, flat, imperative loop nest that visits every
statement instance of the whole program in exactly the order the schedule specifies: no recursion,
no memoization flags, one direct array write per instance. This is the classic polyhedral
code-generation model (what Pluto/PPCG/CLooG-style tools emit) and is a real behavioral shift, not
just a new flag on `WriteC`.

`docs/design.md` currently states: *"Out of scope for this port: the `alphaz`/GeCoS scheduling
search, tiling, memory-mapping, and reduction-simplification machinery. This targets the
demand-driven `WriteC` codegen path only."* This work is a deliberate, explicit expansion of that
stated scope — `docs/design.md` should be updated once this lands.

## 2. Decisions already made (recap)

Four forks were resolved before writing this spec:

1. **Schedule format: raw ISL union map text.** Not a schedule tree, not a new hand-rolled DSL.
   `isl_ast_build_node_from_schedule_map` — which every loop `WriteC` generates already goes
   through — natively consumes a union map. Schedule trees would need a large new
   `isl_schedule_node_*` binding surface in `isl-sys`/`isl` that doesn't exist today; a custom DSL
   would need new parsing/design work for no real semantic gain over isl's own well-known syntax.
   `main.rs`'s own doc comment already notes this port deliberately avoided coupling to the legacy
   `alpha.targetmapping.xtext` grammar — this keeps that.
2. **Statement granularity: one statement per (output/local) variable.** Matches `WriteC`'s
   existing per-variable grouping (`equations_by_var` in `writec.rs`) — piecewise cases (a variable
   defined across multiple `SystemBody`s) stay merged into one statement via the same ternary
   selection `gen_eval_function` already does, not split into separate schedulable pieces.
3. **Reduce bodies are independently schedulable.** A `reduce(...)`'s own summation isn't left as
   an isl-auto-ordered internal loop (as `WriteC`'s `gen_reduce` does today) — its iteration order
   is controlled by the target mapping too. See §4.2 — this turns out to compose cleanly with
   decision 2 via a change to `normalize_reduction.rs`, not a bolt-on.
4. **Legality is checked, not assumed.** An illegal schedule (one that reorders a real dependence)
   must be rejected with a diagnostic, not silently miscompiled. See §6 — this turns out to be
   materially simpler than general polyhedral dependence analysis because Alpha is a pure
   single-assignment language: the true dependences are already explicit in the IR as `Dependence`
   nodes, nothing needs to be *inferred* from array-access overlap the way it would in an imperative
   language.

## 3. Current architecture, in the two places this design diverges from it

- **`ir::System`** (`alpha-transform/src/ir.rs`): `inputs`/`outputs`/`locals: Vec<Variable>`, each
  with a `domain: Set`; `bodies: Vec<SystemBody>`, each `{ domain: Set, equations: Vec<Equation> }`.
  A variable can be defined by equations across multiple bodies (piecewise).
- **`normalize_reduction::apply`** already hoists every *top-level* `Reduce` (one not nested inside
  another `Reduce`'s own body) out of its enclosing equation into a fresh synthetic local variable
  + equation (`R_NR0`, `R_NR1`, ... today) whose `expr` is the `Reduce` node itself, unwrapped. It
  recurses through `Case`/`If`/`Dependence`/`Restrict`/`Select`/`MultiArg`/`Binary`/`Unary` looking
  for the first `Reduce` on each path, so this fires regardless of how deeply the reduce is nested
  inside conditionals — it only *doesn't* fire for a `Reduce` nested directly inside another
  `Reduce`'s `body` (an explicitly acknowledged, carried-over limitation; see §10).
- **`isl-sys`** wraps `isl_.*` broadly (bindgen allowlist) over a fixed header subset
  (`wrapper.h`) that already includes `union_map.h`, `set.h`, `map.h`, `space.h`. Every raw FFI
  function this design needs (`isl_union_map_read_from_str`, `isl_union_map_union`,
  `isl_union_map_extract_map`, `isl_set_set_tuple_name`, `isl_map_is_injective`, `isl_map_lex_ge`,
  ...) **already exists in the generated bindings** — none of it is new to `isl-sys`. Only the safe
  `isl` wrapper crate needs new methods (§7). This is the same pattern the crate already follows for
  every existing `Set`/`Map`/`Aff` method.

## 4. Statement model

### 4.1 Ordinary (non-reduction) statements

One statement per output/local variable `V`, domain = `V.domain` (the `a`-dimensional space
already computed by `alpha-model`). Its body, at a given point, is exactly what
`gen_eval_function` computes today (a ternary chain selecting among `V`'s piecewise equations by
guard domain) — *minus* the flag-check/memoization wrapper, and with every `Dependence` read
becoming a direct array access instead of an `eval_<name>(...)` call (§7.2).

### 4.2 Reduction statements — split into a pair

Because reduce bodies are independently schedulable (decision 3), every `R_NR<n>` local produced
by `normalize_reduction::apply` becomes **two** statements instead of one, sharing `R_NR<n>`'s own
storage:

- **`<name>__init`**, domain = the reduce's ambient (`a`-dimensional) domain — same as today's
  `R_NR<n>` domain. Body: `R_NR<n>[i...] = <neutral element for the operator>` (`0` for `+`/`sum`/
  `or`, `1` for `*`/`prod`/`and`, `INFINITY`/`-INFINITY` for `min`/`max`, matching `gen_reduce`'s
  existing `init_val` table).
- **`<name>__reduce`**, domain = the reduce's *full* `(a+b)`-dimensional domain — exactly
  `context_domain ∩ expression_domain` of the reduce's `body` sub-expression, i.e. what
  `gen_reduce` today calls `full_domain`. Body: `R_NR<n>[i...] = combine(R_NR<n>[i...], <value of
  body at this point>)`, where `combine` is the same operator table `gen_reduce` already has
  (`reduceVar + value`, `min(reduceVar, value)`, an external combiner call, ...).

This eliminates `gen_reduce`'s own internal `isl_ast_build` call entirely for this backend — the
`(a+b)`-dim loop over the reduce's own new dimensions is no longer a private sub-loop built inside
another statement's generated function; it's just more of the *one* whole-program schedule (§7.1),
with its own entry in the target mapping like any other statement.

**Legality note**: `<name>__reduce` reads-and-writes the same array cell `R_NR<n>[i...]` its own
`<name>__init` instance (and every other `<name>__reduce` instance sharing the same ambient `i...`)
also touches. This is a genuine RAW dependence — `<name>__init(i)` must be scheduled before every
`<name>__reduce(i, j...)` — captured directly by the reduce node's existing `projection: MultiAff`
field (`(i,j) -> i`), fed into the same lex-order legality check every other dependence edge uses
(§6.2); no special-cased "reduction dependence" logic needed.

### 4.3 Statement naming and discoverability

`R_NR0`/`R_NR1`/... are counter-based names, deliberately *not* predictable from source today —
`normalize_reduction.rs`'s own doc comment says so explicitly, on the stated assumption that
"codegen never surfaces these names to a user." **That assumption breaks with this feature**: a
human hand-writing a target mapping needs a name for every reduce statement, and a global counter
assigned by pass-ordering isn't something they can predict from the `.alpha` source.

Two changes, both required, not optional polish:

1. **Deterministic naming.** Revert to the upstream Java system's own convention (which the port's
   doc comment explicitly says it diverged from only because it didn't need to matter yet): name an
   extracted reduce after its *enclosing equation's target variable* (`B` → `B_NR` / `B_NR0`,
   `B_NR1`, ... only on an actual same-equation collision), not a whole-system counter. This makes
   `<targetvar>_NR<n>__init` / `<targetvar>_NR<n>__reduce` derivable by reading the source.
2. **A `--list-statements` introspection mode on `alphac`.** Prints every schedulable statement name
   for a given `.alpha` file, its domain (in isl set syntax, so it can be pasted straight into a
   target mapping), and the default identity schedule it'll get if left unmentioned. This is cheap
   (it's just running the pipeline through statement-construction and stopping before codegen) and
   is the practical answer to "how do I even know what to write" — hand-deriving `R_NR`-style names
   and exact `(a+b)`-dim reduce domains from source by eye doesn't scale past trivial examples.

## 5. Target mapping format

A target mapping is **one ISL union map**, read via `isl_union_map_read_from_str`, with one map
per statement, keyed by tuple name = statement name (§4.3):

```
{
  R_NR0__init[i]    -> [i, 0];
  R_NR0__reduce[i,j] -> [i, 1, j];
  B[i]              -> [i, 2];
}
```

(Worked example: `foo`'s prefix-sum-style system, `B[i] = reduce(+, (i,j->i), {:0<=j<i}: A[j])` —
this is the sequential, textbook order: for each `i`, initialize, then accumulate over increasing
`j`, then copy out to `B`.)

Rules, all checked at load time with a diagnostic on violation:

- **Every statement maps into one shared schedule space of fixed width.** All tuples' range
  dimensionality must agree (padding with a constant, as `[i, 2]` above implicitly does relative to
  `[i, 1, j]`, is the normal way to interleave statements of different dimensionality — this is
  standard practice for map-based schedules, not an isl quirk). Rejected otherwise, rather than
  silently doing something isl-dependent and hard to predict.
- **A statement absent from the text defaults to its own identity schedule** — `V[i,j,...] ->
  [i,j,...]`, i.e. today's `WriteC` behavior for that one statement — padded to the shared width.
  This lets a target mapping be partial: schedule only the statements you care about, leave the
  rest at plain lexicographic order. (An empty/omitted target mapping altogether ⇒ every statement
  gets its identity schedule ⇒ `ScheduledC` degenerates to a flat, unmemoized, lexicographic-order
  generator — a useful reference point in its own right, distinct from `WriteC`'s recursive one.)
- **A statement present in the text must be total and injective on its own domain**: every point of
  `V.domain` needs a schedule value (a partial map for a *mentioned* statement is an error, not a
  silent partial-identity-fallback — falling back only happens at whole-statement granularity), and
  no two distinct points may map to the same schedule-space point (`isl_map_is_injective` — two
  instances colliding on one timestamp makes their relative order, and hence correctness of a
  read-modify-write reduce step, undefined).
- **Unknown tuple names are an error** (typo protection) — `--list-statements` (§4.3) is the
  intended way to get the exact valid name set.

## 6. Legality checking

### 6.1 Why this is simpler here than in general polyhedral compilers

A general array-language polyhedral compiler has to *infer* true dependences by intersecting
read/write access relations across the whole iteration space (`isl_union_map_compute_flow` or
similar) because the same array cell can be written many times from different statements/iterations
and only *some* pairs are actually a true producer→consumer edge once transitive same-value
overwrites are accounted for.

Alpha is single-assignment: every array cell has exactly one defining equation (piecewise
sub-domains are disjoint by construction — `alpha-model`'s own well-formedness checking already
guarantees this upstream of codegen). So the true dependence relation isn't something to infer at
all — it's already sitting in the IR as `Dependence { function, operand: Variable(name) }` nodes.
Legality checking here is a direct, mechanical translation of already-explicit data through isl's
set/map algebra, not a new analysis.

### 6.2 The check

Build one dependence edge per `Dependence` node reachable in a statement's contributing
expression(s) (for `<name>__reduce`, this means walking the reduce's own `body` sub-expression, not
the whole enclosing equation), plus one synthetic edge per reduce pair
(`<name>__reduce → <name>__init`, dependence function = the reduce's own `projection: MultiAff`,
per §4.2). Each edge is `(consumer_stmt, dep_fn, producer_stmt)` where `dep_fn: consumer.domain ->
producer.domain`.

For each edge, with `S_c`/`S_p` the two statements' own (possibly defaulted-identity) schedule
maps into the shared schedule space:

> the set `{ p ∈ consumer.domain : S_p(dep_fn(p)) ≥_lex S_c(p) }` must be empty.

i.e. the producer instance a consumer instance reads must land strictly earlier in schedule order.
Isl building blocks: `dep_fn` composed with `S_p` via `Map::apply_range` (already exists in
`isl/src/map.rs`), a `Map::lex_ge`-style universal ordering relation on the shared schedule space
(new — `isl_map_lex_ge`, already raw-bound), intersect/`is_empty` (both already exist). A non-empty
violation set becomes a diagnostic naming both statements; extracting a concrete counterexample
point (`Set::sample_point` or similar) is a nice-to-have deferred past v1, not required for a useful
error message.

### 6.3 What this does *not* catch

Inputs need no legality check (no producer statement — they're live data, always "already
available"). `Select`/`IndexPolynomial`/`argreduce` are unsupported by this backend the same way
they're unimplemented in `WriteC` today (§10) — no dependence edges are derived through them because
no codegen exists for them yet either. A `Reduce` nested directly inside another `Reduce`'s body
(the one case `normalize_reduction` doesn't hoist) stays opaque to legality checking, same
carried-over limitation as §4.

## 7. Code generation architecture

### 7.1 One whole-program AST build, not N per-variable driver loops

`WriteC` calls `AstBuild::generate` many times: once per `Reduce` (its own private summation loop)
and once per output (`gen_eval_loop`'s driver loop). `ScheduledC` calls it **once**, over the union
of every statement's `(domain, schedule)` pair — this is exactly what a map-based union schedule is
for. The result is a single `AstNode` tree — `For`/`If`/`Block`/`User` nodes interleaved however the
schedule causes isl to fuse/split loops across statements — that becomes the entire body of the
generated driver function. There's no separate per-output loop section and no per-reduce helper
function in the output C at all.

### 7.2 Walking the AST

A recursive walker (new, replacing `gen_eval_loop`'s current "peel a flat chain of `For`s" logic,
which assumed a single-statement identity schedule and can't handle a real fused/branching tree):

- `For` → `Stmt::For` (iterator/init/cond/inc exactly as today, via `for_iterator`/`for_init`/
  `for_cond`/`for_inc`, unchanged).
- `If` → `Stmt::If`, recursing into `if_then`/`if_else`.
- `Block` → flatten `block_children()` into the surrounding `Vec<Stmt>`.
- `User` → the interesting case. `user_expr()` is an isl call expression: `op_args()[0].id_name()`
  is the statement's tuple name; `op_args()[1..]`, rendered via `to_string_fmt(Format::C)`, are that
  statement's own index values *as C expressions of the enclosing loop iterators* (not necessarily
  bare identifiers — under a non-identity schedule they can be arbitrary affine combinations, e.g.
  after a skew or permutation). Emit a small prologue binding each to a local (`long i = <expr0>;
  long j = <expr1>; ...`), then dispatch to that statement's own body codegen using those names —
  reusing `gen_value`/`gen_case`/`gen_binary`/`gen_unary`/`gen_multi_arg`/`gen_index_function`
  essentially unchanged (they're already pure functions of `names: &[String]`, not of *how* those
  names got bound). This declare-then-reuse-the-same-converters pattern is the standard technique
  map-based-schedule codegens use (PPCG does the same thing) — it's what makes reusing `WriteC`'s
  expression layer possible at all instead of writing a second one.

### 7.3 What changes in the reused expression converters

- **`gen_dependence`**: currently `Role::Input` reads via the interface access macro
  (`name(idx...)`) but `Role::Output`/`Role::Local` call `eval_name(idx...)`. For `ScheduledC`,
  *every* role reads via its access macro — the `Role`/eval-call distinction disappears for reads.
  (The macros themselves, `interface_access_macro`/`flat_access_macro` in `layout.rs`, are unchanged
  — this only changes what `gen_dependence` emits, not how storage is laid out or sized.)
- **`gen_reduce`** is not reused at all — the reduce pair split (§4.2) means a `<name>__reduce`
  leaf's value is just `gen_value` on the reduce's `body` sub-expression under the `(a+b)`-dim names
  bound at that `User` node; no internal `AstBuild` call, no `reduce<N>` helper function, no
  primed-name juggling.
- **No flag arrays, no `eval_<Var>` functions, no self-dependence runtime check.** The schedule
  (once past legality checking, §6) guarantees single-pass causal order, so the entire
  `NOT_EVALUATED`/`IN_PROGRESS`/`EVALUATED` mechanism `WriteC` needs for recursion has no reason to
  exist here. This is a real, intentional loss of a runtime safety net in exchange for compile-time
  legality checking — flagged explicitly as the tradeoff it is, not a silently dropped feature.
- Storage allocation (`flat_alloc_stmts`, `layout::FlatBounds`) is unchanged — every output/local
  still gets exactly the storage it gets today; `<name>__init`/`<name>__reduce` are two *schedule*
  identities sharing one *storage* identity (`R_NR<n>`'s own array), not two arrays.

**Recommendation**: factor the reused pieces (`gen_value` and everything it calls, `ambient_build`,
`pick_names*`, the operator tables) out of `writec.rs` into a shared module (e.g.
`alpha-codegen/src/expr.rs`) both backends depend on, rather than forking ~500 lines. Worth doing as
its own first step so `writec.rs`'s existing tests keep passing unchanged throughout.

## 8. New `isl` wrapper methods needed

All of these call already-bound raw FFI (§3) — **zero changes to `isl-sys`/`wrapper.h`/`build.rs`**,
purely new safe methods on existing types.

| Method | Backs | Used for |
|---|---|---|
| `UnionMap::read_from_str` | `isl_union_map_read_from_str` | parsing the target mapping text |
| `UnionMap::union` | `isl_union_map_union` | combining per-statement fragments + identity defaults |
| `UnionMap::extract_map` | `isl_union_map_extract_map` | pulling one statement's schedule fragment back out (for legality checking and identity-default detection — returns empty, not an error, if that tuple is absent) |
| `Set::set_tuple_name` | `isl_set_set_tuple_name` | tagging each statement's domain with its statement name before anything tuple-name-keyed touches it |
| `Map::is_injective` | `isl_map_is_injective` | schedule well-formedness (§5) |
| a `lex_ge`-style universal ordering relation constructor on a space | `isl_map_lex_ge` | legality check (§6.2) |

`Map::apply_range`, `Set::is_empty`, `Set::union`, `MultiAff::into_map` and everything else this
design leans on already exist in `isl/src/{map,set,aff}.rs` today.

## 9. Public API / CLI surface

- New `alpha_codegen::generate_scheduled_system(system: &ir::System, schedule_text: &str) ->
  Result<String>` alongside the existing `generate_system` (unchanged; `WriteC` stays the default,
  unaffected backend). `schedule_text` may be empty (⇒ every statement defaults to identity, §5).
- `CodegenError` gains variants for schedule-parse and legality failures (or these route through a
  new sibling error type — open question, see §12).
- `alphac` gets:
  - `--schedule <path>` (optional): read the target mapping from a file, select the `ScheduledC`
    backend instead of `WriteC`. Omitted ⇒ today's `WriteC` behavior, completely unchanged.
  - `--list-statements`: print every schedulable statement's name and domain for the given `.alpha`
    file and exit, without generating code (§4.3).

## 10. Explicit v1 non-goals

- **No tiling, no parallel/unroll/separate loop-type annotations** — the target mapping is a pure
  ordering (a map into a logical time space), nothing about *how* isl should realize a given band
  (the legacy `alpha.targetmapping.xtext` grammar's `tile-band`/`isolate`/`LoopTypeSpecification`
  machinery). Consistent with `docs/design.md`'s existing stated scope boundary on tiling.
- **No automatic scheduling.** This is codegen *given* a schedule, not a scheduler that derives one
  (no `isl_schedule_constraints`-driven auto-scheduling, no cost model).
- **`Select`/`IndexPolynomial`/`argreduce`** stay unsupported, matching `WriteC`'s own current gaps
  (§10 of `writec.rs`'s doc comment) — no new work here to close those.
- **A `Reduce` nested directly inside another `Reduce`'s body** stays opaque to both scheduling and
  legality checking, inheriting `normalize_reduction.rs`'s existing single-pass limitation.
- **Counterexample extraction for a legality violation** (a concrete violating index tuple, not just
  "these two statements conflict") is deferred.
- **`UseEquation`** (subsystem calls) stays unsupported, matching `WriteC` and the source system's
  own `WriteC`.

## 11. Suggested phasing

1. Extract shared expression-codegen (`gen_value` and friends) out of `writec.rs` into a module both
   backends use, with no behavior change — a pure refactor, verified by the existing `WriteC` test
   suite staying green.
2. `isl` wrapper additions (§8) + smoke tests against real isl, independent of the rest.
3. Deterministic reduce naming in `normalize_reduction.rs` (§4.3) — small, self-contained, and
   `WriteC`-visible-but-harmless (only changes generated internal names, not behavior).
4. Statement model + target-mapping parsing/validation (§4, §5) — produces a validated, fully
   fused `UnionMap` plus diagnostics; no codegen yet.
5. Legality checker (§6) — built and tested against step 4's output independently of codegen.
6. AST walker + statement-body codegen (§7) — the new `scheduledc.rs`.
7. `alphac` CLI wiring (`--schedule`, `--list-statements`) + a new
   `tests/alpha-language-fixtures`-style fixture corpus for this backend + `docs/design.md` update.

## 12. Open questions for the next iteration

- Exact error-type shape for schedule-parse vs. legality-violation vs. isl failures (new enum? new
  variants on `CodegenError`?).
- Exact text format for `--list-statements` output (plain text vs. something machine-readable a
  future tool could round-trip).
- Whether `<name>__init`/`<name>__reduce`'s `__` separator is likely to collide with real Alpha
  identifiers (Alpha's own identifier grammar — worth a quick check before settling on it).
- Whether the shared-schedule-space-width rule (§5) should auto-pad shorter tuples instead of
  rejecting width mismatches outright — rejecting is simpler and more predictable to start with, but
  worth revisiting once real target mappings get written by hand.
