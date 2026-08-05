# Scheduled codegen: implementation spec

Status: ready for implementation. Target branch: `louis/scheduled-codegen`.

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
stated scope; `docs/design.md` must be updated as part of §12's phasing.

**Scheduling is only ever defined over the normalized IR.** Normalizing hoists every top-level
`Reduce` into its own pair of statements (§4.2) via two sequential passes — `normalize::apply` then
`normalize_reduction::apply` (§10.1) — before a target mapping can name every schedulable unit
correctly. There is no such thing as "the schedule for the program before normalization";
normalization isn't a pass a target mapping schedules around, it's a precondition for the target
mapping to be well-formed at all. §5 covers what that means for tooling.

## 2. Design decisions

1. **Schedule format: raw ISL union map text.** Not a schedule tree, not a new hand-rolled DSL.
   `isl_ast_build_node_from_schedule_map` — which every loop `WriteC` generates already goes
   through — natively consumes a union map. Schedule trees would need a large new
   `isl_schedule_node_*` binding surface in `isl-sys`/`isl` that doesn't exist today; a custom DSL
   would need new parsing/design work for no real semantic gain over isl's own well-known syntax.
   `main.rs`'s own doc comment already notes this port deliberately avoided coupling to the legacy
   `alpha.targetmapping.xtext` grammar — this keeps that.
2. **Statement granularity: one statement per (output/local) variable, scoped to its containing
   system.** Matches `WriteC`'s existing per-variable grouping (`equations_by_var` in `writec.rs`)
   — piecewise cases (a variable defined across multiple `SystemBody`s) stay merged into one
   statement via the same ternary selection `gen_eval_function` already does, not split into
   separate schedulable pieces. A variable name is only meaningful relative to the one `ir::System`
   (§3) that declares it — the same relationship a parameter name has to its enclosing function, not
   a global identifier. Two systems that each happen to declare a variable named `B` are exactly as
   unrelated as `foo(int Y)`'s `Y` and `bar(int Y)`'s `Y`; nothing about the string `"B"` carries
   meaning across that boundary. This matters because the notebook (§5) can hold several systems in
   memory side by side as separate variables — implementations must not treat statement names as
   identifiers in some shared or notebook-wide namespace (§4's statement model, §6's target-mapping
   tuple names, §5's notebook all depend on this).
3. **Reduce bodies are independently schedulable.** A `reduce(...)`'s own summation isn't left as
   an isl-auto-ordered internal loop (as `WriteC`'s `gen_reduce` does today) — its iteration order
   is controlled by the target mapping too. See §4.2; this composes with decision 2 via a change to
   `normalize_reduction.rs`.
4. **Legality is checked, not assumed.** An illegal schedule (one that reorders a real dependence)
   must be rejected with a diagnostic, not silently miscompiled. See §7. This is materially simpler
   than general polyhedral dependence analysis because Alpha is a pure single-assignment language:
   the true dependences are already explicit in the IR as `Dependence` nodes, nothing needs to be
   *inferred* from array-access overlap the way it would in an imperative language.
5. **Normalization is a hard precondition for scheduling, not an optional earlier stage.**
   `alphac` never accepts a target mapping against a pre-normalization system, and there is no way
   to skip normalizing before scheduled codegen runs — every path that consumes a target mapping
   (§10) normalizes first, unconditionally. Normalizing is two genuinely separate Rust-level passes
   run in sequence — `normalize::apply` then `normalize_reduction::apply` (§1, matching the upstream
   Java system's own order: `Normalize.apply` then `NormalizeReduction.apply` in `WriteC.xtend`'s
   `preprocess()`) — but the caller only ever sees one conceptual step: `alpha.normalize()` (§10.1)
   bundles both, so nothing downstream of it has to sequence two calls itself. Since normalizing
   changes statement identity (hoisting reduces into their own statements, §4.2), "what should the
   schedule look like before normalization" isn't a well-formed question — there is no such state to
   design for.
6. **Tooling is a Jupyter notebook.** A standard Python kernel plus IPython cell magics expose five
   pipeline stages — read, normalize, print, schedule, generate (§5.2) — as notebook cells, backed
   by a new PyO3 binding crate (`alpha-py`, §10.1). Two properties this depends on:
   - **Statefulness comes for free.** Re-running `schedule`/`generate` against the same normalized
     IR without reparsing is just "re-run this cell using a variable an earlier cell defined" — the
     thing every notebook already does.
   - **The normalized-IR precondition (§5.1) is a type distinction for `schedule`/`generate` only —
     not for `print`.** Printing an IR's current state has no normalization requirement; it just
     shows whatever the IR looks like right now, pre- or post-normalization. `System` (freshly
     parsed) and `NormalizedSystem` (post-normalization) are distinct Python types (§10); only
     `schedule`/`generate` are gated on it — passing a bare `System` to either is a `TypeError` from
     the binding, before any Rust code runs. `__repr__` is defined on all three types (`System`,
     `NormalizedSystem`, `ScheduledSystem`) and callable at any time, on whichever value is held.

   Every pipeline stage is a pure function from one immutable value to a new one (§5.2, §10) — not a
   command that mutates shared state in place. There is no "current schedule" to track or reset: a
   rejected `schedule()` call simply never produces a new object.

## 3. Current architecture, and what this design changes

- **`ir::System`** (`alpha-transform/src/ir.rs`): `inputs`/`outputs`/`locals: Vec<Variable>`, each
  with a `domain: Set`; `bodies: Vec<SystemBody>`, each `{ domain: Set, equations: Vec<Equation> }`.
  A variable can be defined by equations across multiple bodies (piecewise).
- **`normalize::apply` and `normalize_reduction::apply` are two separate passes; neither invokes the
  other.** The upstream Java system's demand-driven backend (`WriteC.xtend`'s `preprocess()`) runs
  `Normalize.apply` then `NormalizeReduction.apply`, in that order. **`alphac`'s current `main.rs`
  runs them in the opposite order** (`normalize_reduction::apply` before `normalize::apply`), and
  `alpha-transform/README.md` states a correctness rationale for that order (a `Dependence` directly
  wrapping a bare `Reduce` needs somewhere else to live before `normalize`'s own rewriting touches
  it). **This work corrects `main.rs` to match upstream's order** as part of its phasing (§12) —
  `WriteC`'s own fixture corpus must stay green after the fix, verifying the change is output-neutral
  for the demand-driven path. `normalize_reduction::apply` itself hoists every *top-level* `Reduce`
  (one not nested inside another `Reduce`'s own body) out of its enclosing equation into a fresh
  synthetic local variable + equation (`R_NR0`, `R_NR1`, ... today) whose `expr` is the `Reduce` node
  itself, unwrapped. It recurses through `Case`/`If`/`Dependence`/`Restrict`/`Select`/`MultiArg`/
  `Binary`/`Unary` looking for the first `Reduce` on each path, so this fires regardless of how
  deeply the reduce is nested inside conditionals — it only *doesn't* fire for a `Reduce` nested
  directly inside another `Reduce`'s `body` (an explicitly acknowledged limitation; see §11).
- **`isl-sys`** wraps `isl_.*` broadly (bindgen allowlist) over a fixed header subset
  (`wrapper.h`) that already includes `union_map.h`, `set.h`, `map.h`, `space.h`. Every raw FFI
  function this design needs (`isl_union_map_read_from_str`, `isl_union_map_union`,
  `isl_union_map_extract_map`, `isl_set_set_tuple_name`, `isl_map_is_injective`, `isl_map_lex_ge`,
  ...) **already exists in the generated bindings** — none of it is new to `isl-sys`. Only the safe
  `isl` wrapper crate needs new methods (§9). This is the same pattern the crate already follows for
  every existing `Set`/`Map`/`Aff` method.

## 4. Statement model

### 4.1 Ordinary (non-reduction) statements

One statement per output/local variable `V`, domain = `V.domain` (the `a`-dimensional space
already computed by `alpha-model`). Its body, at a given point, is exactly what
`gen_eval_function` computes today (a ternary chain selecting among `V`'s piecewise equations by
guard domain) — *minus* the flag-check/memoization wrapper, and with every `Dependence` read
becoming a direct array access instead of an `eval_<name>(...)` call (§8.2).

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
another statement's generated function; it's just more of the *one* whole-program schedule (§8.1),
with its own entry in the target mapping like any other statement.

**Legality note**: `<name>__reduce` reads-and-writes the same array cell `R_NR<n>[i...]` its own
`<name>__init` instance (and every other `<name>__reduce` instance sharing the same ambient `i...`)
also touches. This is a genuine RAW dependence — `<name>__init(i)` must be scheduled before every
`<name>__reduce(i, j...)` — captured directly by the reduce node's existing `projection: MultiAff`
field (`(i,j) -> i`), fed into the same lex-order legality check every other dependence edge uses
(§7.2); no special-cased "reduction dependence" logic needed.

### 4.3 Statement naming and discoverability

`R_NR0`/`R_NR1`/... are counter-based names, deliberately *not* predictable from source today —
`normalize_reduction.rs`'s own doc comment says so explicitly, on the stated assumption that
"codegen never surfaces these names to a user." **That assumption breaks with this feature**: a
human hand-writing a target mapping needs a name for every reduce statement, and a global counter
assigned by pass-ordering isn't something they can predict from the `.alpha` source.

Two changes, both required:

1. **Deterministic naming.** Match the upstream Java system's own convention: name an extracted
   reduce after its *enclosing equation's target variable* (`B` → `B_NR` / `B_NR0`, `B_NR1`, ...
   only on an actual same-target-variable collision), not a whole-system counter. This makes
   `<targetvar>_NR<n>__init` / `<targetvar>_NR<n>__reduce` derivable by reading the source.
2. **Collision avoidance against all existing names, not just other reduces.** Alpha's identifier
   grammar (`\^?[a-zA-Z_][a-zA-Z_0-9]*` in `alpha-syntax/src/token_kind.rs`) permits underscores
   freely, so a real Alpha variable literally named e.g. `B_NR0__init` is syntactically legal and
   could collide with a synthesized statement name. The naming rule in point 1 must check the
   candidate name (`<targetvar>_NR<n>`, and its derived `__init`/`__reduce` forms) against every
   existing variable/local/statement name in the system — not only other reduces hoisted from the
   same target variable — and increment `<n>` until the candidate is unique.
3. **A way to see the normalized statement set before writing a schedule against it.**
   Hand-deriving `R_NR`-style names and exact `(a+b)`-dim reduce domains from source by eye doesn't
   scale past trivial examples — see §5 for the notebook-level equivalent (a `NormalizedSystem`
   value's own `repr`) that does this instead.

All of the above is scoped to one system, consistent with decision 2 (§2): `<targetvar>_NR<n>`
naming and the identity-or-current-schedule skeleton `repr` shows (§5.2) are computed relative to
whichever `System`/`NormalizedSystem` produced them, never relative to a notebook-wide namespace. A
notebook holding two systems that each define a variable `B` gets two independent `B_NR0__init`-style
names, one per system — never one shared name to collide over.

## 5. Workflow: a Jupyter notebook

### 5.1 The precondition, and how the notebook enforces it

A target mapping is keyed by statement names (§4.3) and shaped by statement domains (§4.2) that
only exist once normalizing has run both its passes — `normalize::apply` and
`normalize_reduction::apply` (§1, §2 decision 5). A reduce isn't one schedulable unit until
normalization has split it into `<name>__init`/`<name>__reduce`, and the `(a+b)`-dim domain of the
`__reduce` half is an isl set the compiler computes, not something written down anywhere in the
source `.alpha` file. So a human writing a target mapping by hand is, structurally, always writing
it against the *output* of normalization, never the input. This rules out designing scheduled
codegen as something layered on top of an otherwise-unmodified pipeline where normalization is "just
another pass that happens to run before codegen."

The precondition applies only to `schedule`/`generate`, the two operations that need a well-formed,
fully-split statement set (§4.2) to make sense against — it does not apply to `print`. `print` isn't
consuming a target mapping, it's just displaying whatever the loaded IR looks like right now: there
is a well-defined pre-normalization skeleton (one entry per variable, no reduce splitting — a
`Reduce` is still nested inside its enclosing variable's own equation) exactly as there's a
well-defined post-normalization one (§4.2's `<name>__init`/`<name>__reduce` pairs), so nothing stops
printing either one.

The notebook's Python surface (§10) gives freshly-parsed and post-normalization systems genuinely
different types — `System` and `NormalizedSystem` — but both (and `ScheduledSystem`) define
`__repr__`, callable at any time with no precondition (§10.1). Only `NormalizedSystem.schedule(...)`
and `generate(...)` are type-gated: `normalize(sys: System) -> NormalizedSystem` is the only function
that produces a `NormalizedSystem`, and `schedule`/`generate` only accept a `NormalizedSystem` or the
`ScheduledSystem` it produces — passing a bare `System` to either is a Python `TypeError` raised by
the binding before any Rust code runs. This enforcement lives at the PyO3 binding layer:
`PyNormalizedSystem` is a distinct wrapper struct, not a generic parameter, so
`alpha_transform::ir::System` itself remains a single Rust type (§13 covers whether to push the
distinction further down). `alpha_codegen::generate_scheduled_system` (§10) documents the
precondition in its own doc comment too, since the Rust API itself has no way to see the
Python-level type distinction.

Statement names (§4.3) are meaningful only relative to the one `System`/`NormalizedSystem` they came
from (§2 decision 2). The `%%schedule` magic (§5.2) is explicit about this by construction — its
magic line names the `NormalizedSystem` the target-mapping text is checked against. A target mapping
written against one system's `B` cannot be reused against a different system's `B`, any more than an
argument list written for `foo(int Y)` could be reused to call `bar(int Y)`.

### 5.2 The notebook: `%%alpha`, `normalize`, `repr`, `%%schedule`, `generate`

Five pipeline stages — read, normalize, print, schedule, generate — split across two mechanisms:

- **Reading Alpha or target-mapping *source text* happens through IPython cell magics**, because
  that's the part that's its own little language and benefits from syntax highlighting in the
  editor, not from being a Python expression.
- **Everything else is a plain, typed Python function or method** over immutable values, because
  that's the part that's just "take a value, produce a new one" — no new syntax needed, and it's
  what makes the rust-like "transformations return new copies" behavior precise (§10 gives the
  exact signatures).

```
%%alpha sys
input A[N];
B[i] = reduce(+, (i,j->i), {:0<=j<i}: A[j]);
```

This cell magic parses its body as Alpha source (parse + `alpha_model::analyze_system` +
`alpha_transform::lower::lower_system`), and on success binds the result to the notebook variable
named on the magic line (`sys`, here) as an `alpha.System`. Diagnostics from any of those three
steps are reported as the cell's error output, IPython's normal mechanism for a failed cell — no
variable is bound. Re-running `%%alpha sys` with edited source binds a new `System` to the same
name; an old `System` value is never mutated by doing so, it's simply no longer referenced.

```python
sys
```

`System.__repr__` works immediately — no normalization needed just to look at what's loaded (§5.1):
one entry per variable at its identity schedule, with `B`'s `reduce` still nested inside `B`'s own
equation rather than split into its own statement (that split is `normalize`'s job, next):

```
{
  A[i] -> [i];
  B[i] -> [i];
}
```

```python
norm = alpha.normalize(sys)
norm
```

`alpha.normalize` runs `normalize::apply` then `normalize_reduction::apply` (§1) against a *clone* of
`sys`'s underlying IR — two separate Rust passes, bundled into one Python call so nothing downstream
has to sequence them itself — and returns a new `alpha.NormalizedSystem`; `sys` itself is untouched
and still usable. (Cloning an isl `Set`/`Map` is a cheap refcount bump — `isl_set_copy`/
`isl_map_copy` — not a real memory-duplicating deep copy, so "deep-copy-then-modify-the-copy" as an
implementation strategy costs about what "mutate in place" would have; see §10.) Trailing-expression
display is Jupyter's own mechanism for showing a value without an explicit `print`, so a bare `norm`
on a cell's last line prints it: `NormalizedSystem.__repr__` renders the ISL union-map skeleton, one
entry per statement, current or default-identity schedule:

```
{
  R_NR0__init[i]    -> [i, 0];
  R_NR0__reduce[i,j] -> [i, 1, j];
  B[i]              -> [i, 2];
}
```

```
%%schedule sched norm
{
  R_NR0__init[i]     -> [i, 0];
  R_NR0__reduce[i,j] -> [i, 1, j];
  B[i]               -> [i, 2];
}
```

The `%%schedule` magic takes two names on its magic line — the notebook variable to bind (`sched`)
and the `NormalizedSystem` variable to schedule against (`norm`) — and its cell body is target-
mapping text in the §6 format, typically pasted and hand-edited from a `norm`-cell's own `repr`
output above. It's sugar for `sched = norm.schedule(text)`: `NormalizedSystem.schedule` parses the
text, validates it (§6), checks legality against `norm`'s dependences (§7), and — only if all of
that passes — returns a new `alpha.ScheduledSystem`; `norm` is never mutated. A rejected schedule
raises `alpha.ScheduleError` (diagnostic attached) as the cell's error output and binds nothing.
`sched` also has a `repr` showing the same skeleton syntax, reflecting the schedule that was
actually loaded, as a sanity check.

If the magic line's source-system name doesn't resolve to a `NormalizedSystem` in the notebook
namespace (wrong type, or undefined), the magic raises a Python `TypeError`/`NameError` before
attempting to parse the cell body, consistent with every other type-gated operation in §5.1.

```python
code = alpha.generate(sched)
print(code)
```

`alpha.generate` runs `ScheduledC` (§8) and returns the generated C source as a `str`. It also
accepts a bare `NormalizedSystem` directly (skipping `%%schedule` entirely) — consistent with §6's
"an omitted target mapping ⇒ every statement gets its identity schedule" rule, this is just
`generate(norm)` as sugar for `generate(norm.schedule(""))`.

No magics or functions beyond these two cell magics and the handful of Python calls above are
needed for v1. §5.3 covers what's deliberately out of scope.

### 5.3 Out of scope for v1

- **Scripting is headless notebook execution, not a separate code path.** `jupyter nbconvert
  --execute` (or `nbclient` directly) runs a `.ipynb` top to bottom outside a live kernel; comparing
  its cell outputs against checked-in expected output is exactly what the `nbval` pytest plugin
  does. This is v1's non-interactive story and its fixture-test mechanism (§12) — a small corpus of
  notebooks, not a separate script format.
- Not designed here: a dedicated Alpha Jupyter *kernel* (as opposed to cell magics on a standard
  Python kernel), editing a `NormalizedSystem`'s schedule inline without round-tripping through
  `%%schedule` cell text, LSP/editor integration for the magics beyond basic syntax highlighting, or
  auto-repair suggestions for a rejected schedule. None of these are needed to get read → normalize
  → print → schedule → generate working as notebook cells.

## 6. Target mapping format

A target mapping is **one ISL union map**, read via `isl_union_map_read_from_str`, with one map
per statement, keyed by tuple name = statement name (§4.3) of the one `NormalizedSystem` the mapping
is checked against (§5.1) — never compared across different systems (§2 decision 2). Written by
hand, typically starting from a `NormalizedSystem` value's own `repr` output (§5.2):

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

Rules, all checked at load time (`NormalizedSystem.schedule(...)` / the `%%schedule` magic, §5.2)
with a diagnostic on violation:

- **Every statement maps into one shared schedule space of fixed width.** All tuples' range
  dimensionality must agree (padding with a constant, as `[i, 2]` above implicitly does relative to
  `[i, 1, j]`, is the normal way to interleave statements of different dimensionality — this is
  standard practice for map-based schedules, not an isl quirk). Mismatched widths are rejected,
  rather than silently doing something isl-dependent and hard to predict.
- **A statement absent from the text defaults to its own identity schedule** — `V[i,j,...] ->
  [i,j,...]`, i.e. today's `WriteC` behavior for that one statement — padded to the shared width.
  This lets a target mapping be partial: schedule only the statements you care about, leave the
  rest at plain lexicographic order. An empty/omitted target mapping altogether ⇒ every statement
  gets its identity schedule ⇒ `ScheduledC` degenerates to a flat, unmemoized, lexicographic-order
  generator — a useful reference point in its own right, distinct from `WriteC`'s recursive one.
- **A statement present in the text must be total and injective on its own domain**: every point of
  `V.domain` needs a schedule value (a partial map for a *mentioned* statement is an error, not a
  silent partial-identity-fallback — falling back only happens at whole-statement granularity), and
  no two distinct points may map to the same schedule-space point (`isl_map_is_injective` — two
  instances colliding on one timestamp makes their relative order, and hence correctness of a
  read-modify-write reduce step, undefined).
- **Unknown tuple names are an error** (typo protection) — a `NormalizedSystem` value's own `repr`
  (§5.2) is the intended way to get the exact valid name set.

## 7. Legality checking

### 7.1 Why this is simpler here than in general polyhedral compilers

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

### 7.2 The check

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
point (`Set::sample_point` or similar) is deferred past v1 (§13), not required for a useful error
message.

### 7.3 What this does *not* catch

Inputs need no legality check (no producer statement — they're live data, always "already
available"). `Select`/`IndexPolynomial`/`argreduce` are unsupported by this backend the same way
they're unimplemented in `WriteC` today (§11) — no dependence edges are derived through them because
no codegen exists for them yet either. A `Reduce` nested directly inside another `Reduce`'s body
(the one case `normalize_reduction` doesn't hoist) stays opaque to legality checking, same
carried-over limitation as §4.

## 8. Code generation architecture

### 8.1 One whole-program AST build, not N per-variable driver loops

`WriteC` calls `AstBuild::generate` many times: once per `Reduce` (its own private summation loop)
and once per output (`gen_eval_loop`'s driver loop). `ScheduledC` calls it **once**, over the union
of every statement's `(domain, schedule)` pair — this is exactly what a map-based union schedule is
for. The result is a single `AstNode` tree — `For`/`If`/`Block`/`User` nodes interleaved however the
schedule causes isl to fuse/split loops across statements — that becomes the entire body of the
generated driver function. There's no separate per-output loop section and no per-reduce helper
function in the output C at all.

### 8.2 Walking the AST

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

### 8.3 What changes in the reused expression converters

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
  (once past legality checking, §7) guarantees single-pass causal order, so the entire
  `NOT_EVALUATED`/`IN_PROGRESS`/`EVALUATED` mechanism `WriteC` needs for recursion has no reason to
  exist here. This is a real, intentional loss of a runtime safety net in exchange for compile-time
  legality checking.
- Storage allocation (`flat_alloc_stmts`, `layout::FlatBounds`) is unchanged — every output/local
  still gets exactly the storage it gets today; `<name>__init`/`<name>__reduce` are two *schedule*
  identities sharing one *storage* identity (`R_NR<n>`'s own array), not two arrays.

**Implementation note**: factor the reused pieces (`gen_value` and everything it calls,
`ambient_build`, `pick_names*`, the operator tables) out of `writec.rs` into a shared module (e.g.
`alpha-codegen/src/expr.rs`) both backends depend on, rather than forking ~500 lines. Do this as the
first phasing step (§12) so `writec.rs`'s existing tests keep passing unchanged throughout.

## 9. New `isl` wrapper methods needed

All of these call already-bound raw FFI (§3) — **zero changes to `isl-sys`/`wrapper.h`/`build.rs`**,
purely new safe methods on existing types.

| Method | Backs | Used for |
|---|---|---|
| `UnionMap::read_from_str` | `isl_union_map_read_from_str` | parsing the target mapping text |
| `UnionMap::union` | `isl_union_map_union` | combining per-statement fragments + identity defaults |
| `UnionMap::extract_map` | `isl_union_map_extract_map` | pulling one statement's schedule fragment back out (for legality checking and identity-default detection — returns empty, not an error, if that tuple is absent) |
| `Set::set_tuple_name` | `isl_set_set_tuple_name` | tagging each statement's domain with its statement name before anything tuple-name-keyed touches it |
| `Map::is_injective` | `isl_map_is_injective` | schedule well-formedness (§6) |
| a `lex_ge`-style universal ordering relation constructor on a space | `isl_map_lex_ge` | legality check (§7.2) |

`Map::apply_range`, `Set::is_empty`, `Set::union`, `MultiAff::into_map` and everything else this
design leans on already exist in `isl/src/{map,set,aff}.rs` today.

## 10. Public API / package surface

- New `alpha_codegen::generate_scheduled_system(system: &ir::System, schedule_text: &str) ->
  Result<String>` alongside the existing `generate_system` (unchanged; `WriteC` stays the default,
  unaffected backend). **Precondition, documented on the function itself**: `system` must already be
  the output of both normalizing passes — `normalize::apply` and `normalize_reduction::apply` (§1,
  §5.1) — `alpha_codegen` doesn't normalize itself (that lives in `alpha_transform`), so this is a
  doc-comment contract at the Rust level, not something the signature enforces. The Python binding
  below turns this into an enforced type distinction, one layer up.
- `CodegenError` gains variants for schedule-parse and legality failures (exact shape is an
  implementation-time decision — see §13).
- `alphac`'s existing non-interactive entry point (`alphac file.alpha -o file.c`, `WriteC`) gets no
  new CLI surface from this work — no `repl` subcommand, no other new flags. The interactive story
  (§5) lives entirely in the new Python binding below, not in `alphac` itself. (§3's `main.rs`
  pass-order fix is the one change this work makes to the existing `WriteC` path, and it is
  output-neutral.)

### 10.1 `alpha-py`: a new PyO3 binding crate

A new workspace member, `alpha-py`, in the same spirit as the existing `editors/vscode/native`
crate (a napi-rs binding exposing `alpha-syntax`/`alpha-model` diagnostics to the VS Code extension
in-process) — same pattern, different host language: `pyo3` instead of `napi`, built into a wheel
with `maturin` instead of a `cdylib` napi module. Where `editors/vscode/native` only needs to expose
parse/analyze diagnostics, `alpha-py` needs the full read → normalize → schedule → generate surface,
so it depends on `alpha-syntax`, `alpha-model`, `alpha-transform`, and `alpha-codegen`. The Python
package/import name is **`alpha`** (`import alpha`, `pip install alpha`) — check for a PyPI name
collision before any public (non-internal) release.

Three PyO3-wrapped types, each an immutable value wrapping a cloned `alpha_transform::ir::System`
(or, for `ScheduledSystem`, the system plus its validated `isl::UnionMap` schedule) — deliberately
*not* one mutable class with a state enum, so "which stage is this at" is a Python `type()` check,
not a field to inspect:

| Python type | Produced by | Wraps |
|---|---|---|
| `alpha.System` | `%%alpha` cell magic (§5.2) — parse + `analyze_system` + `lower_system` | `ir::System`, pre-normalization |
| `alpha.NormalizedSystem` | `alpha.normalize(sys: System) -> NormalizedSystem` | `ir::System`, post `normalize::apply` then `normalize_reduction::apply` (§1) — two Rust passes, one Python call |
| `alpha.ScheduledSystem` | `NormalizedSystem.schedule(text: str) -> ScheduledSystem` (also driven by the `%%schedule` magic) | the normalized `ir::System` plus a validated (§6), legality-checked (§7) `isl::UnionMap` |

Functions/methods, all pure — clone the receiver's underlying Rust value, run the existing
(in-place-mutating) Rust pass over the clone, wrap the result, never touch the original:

- `alpha.read(path: str) -> System` — same three steps as `%%alpha`, for loading a `.alpha` file
  from disk instead of inline cell text (useful outside a notebook, e.g. from a plain script).
- `alpha.normalize(sys: System) -> NormalizedSystem` — always runs *deep* normalization (`normalize`'s
  `deep: bool` parameter is `true`, matching `alphac/main.rs`'s existing call), since scheduled
  codegen has no use for the shallow, readability-oriented form (polyhedral-object shorthand, named
  cases, auto-restrict) that shallow normalization preserves.
- `NormalizedSystem.schedule(text: str) -> ScheduledSystem` — raises `alpha.ScheduleError` (carrying
  the §6/§7 diagnostic) on a parse, validation, or legality failure; raises nothing into `norm`
  itself, since there's nothing in it to roll back.
- `alpha.generate(system: NormalizedSystem | ScheduledSystem) -> str` — calls
  `alpha_codegen::generate_scheduled_system` under the hood; a bare `NormalizedSystem` is sugar for
  scheduling it with empty text first (§6's identity-schedule default).
- `System.__repr__` / `NormalizedSystem.__repr__` / `ScheduledSystem.__repr__` — all three print at
  any time, with no precondition (§5.1): the ISL union-map skeleton text (§5.2), one entry per
  variable, at each variable's identity schedule unless a `ScheduledSystem`'s own schedule overrides
  it. `System`'s version reflects the pre-normalization statement set (no reduce splitting — a
  `Reduce` is still nested inside its enclosing variable's own equation, not its own statement);
  `NormalizedSystem`'s and `ScheduledSystem`'s reflect the post-normalization one (§4.2).

### 10.2 Notebook integration

- Two IPython cell magics, `%%alpha <var>` and `%%schedule <var> <source-system-var>` (§5.2),
  registered by an `IPython.core.magic.Magics` subclass that ships in the `alpha` Python package and
  self-registers on `import alpha` (the standard `%load_ext`-free pattern most magic-providing
  packages use).
- Syntax highlighting reuses the TextMate grammar the VS Code extension already ships
  (`editors/vscode/syntaxes/alpha.tmLanguage.json`) rather than hand-writing a second grammar.
  JupyterLab 4's editor is CodeMirror 6, which can consume a TextMate grammar through a bridge
  package (e.g. `codemirror-textmate`); a small JupyterLab extension maps the `%%alpha`/`%%schedule`
  magic-line regex to that grammar, the same "magic prefix → language mode" technique `%%html`/
  `%%latex`/`ipython-sql` already use for their own cell magics. The target-mapping half
  (`%%schedule`) ships with no dedicated grammar at first (plain text) — §13 covers whether it's
  worth a second, smaller grammar later.

## 11. Explicit non-goals

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
  "these two statements conflict") is out of scope for v1.
- **`UseEquation`** (subsystem calls) stays unsupported, matching `WriteC`.
- **A dedicated Alpha Jupyter kernel, inline schedule editing, LSP/editor integration beyond basic
  syntax highlighting, auto-repair suggestions** (§5.3) — the notebook (§5.2) is deliberately two
  cell magics plus a handful of plain Python calls, nothing more, for v1.
- **A phantom-typed `System<Normalized>`/`System<Raw>` distinction inside `alpha_transform::ir`
  itself** is out of scope. §5.1/§10.1's PyO3-layer type distinction (`System` vs `NormalizedSystem`)
  already enforces the precondition where it matters (the Python API surface); pushing the same
  distinction into the Rust type itself would be a larger refactor with no v1 caller that needs it.

## 12. Implementation phasing

1. Extract shared expression-codegen (`gen_value` and friends) out of `writec.rs` into a module both
   backends use, with no behavior change — a pure refactor, verified by the existing `WriteC` test
   suite staying green.
2. Fix `alphac/main.rs`'s `WriteC` preprocessing to run `normalize::apply` then
   `normalize_reduction::apply`, matching upstream's order (§3) — verify against `WriteC`'s existing
   fixture corpus that generated output is unchanged.
3. `isl` wrapper additions (§9) + smoke tests against real isl, independent of the rest.
4. Deterministic reduce naming + collision avoidance in `normalize_reduction.rs` (§4.3) — small,
   self-contained, and `WriteC`-visible-but-harmless (only changes generated internal names, not
   behavior).
5. Statement model + target-mapping parsing/validation (§4, §6) — produces a validated, fully
   fused `UnionMap` plus diagnostics; no codegen yet.
6. Legality checker (§7) — built and tested against step 5's output independently of codegen.
7. AST walker + statement-body codegen (§8) — the new `scheduledc.rs`.
8. `alpha-py` (§10) — the PyO3 binding crate: `System`/`NormalizedSystem`/`ScheduledSystem` plus
   `read`/`normalize`/`schedule`/`generate`, a thin wrapper over steps 1–7 plus the existing
   `parse`/`analyze_system`/`lower_system` calls `main.rs` already makes — no new compiler logic, no
   parallel code path.
9. The `%%alpha`/`%%schedule` IPython magics and JupyterLab syntax highlighting (§5.2, §10.2) —
   depends on step 8's Python types existing. Plus a fixture corpus of notebooks + `nbval`-checked
   expected output (§5.3), and the `docs/design.md` update (§1).

## 13. Deferred beyond v1

These are decided deferrals, not open questions blocking implementation:

- **Exact error-type shape** for schedule-parse vs. legality-violation vs. isl failures (new enum?
  new variants on `CodegenError`? how the Rust-level error maps to `alpha.ScheduleError`'s fields on
  the Python side) is an implementation-time decision within §10's stated constraints, not something
  this spec needs to pin down further.
- **Counterexample extraction** for a legality violation (§7.2, §11).
- **Auto-padding shorter tuples** in the shared schedule space (§6) instead of rejecting width
  mismatches — rejecting is v1's behavior; revisit once real target mappings get written by hand.
- **A phantom-typed `System<Normalized>` marker inside `alpha_transform::ir`** (§11) — the PyO3-layer
  type distinction already covers v1's needs.
- **A native CodeMirror 6/Lezer grammar** for JupyterLab syntax highlighting, if the TextMate bridge
  (§10.2) proves not to be a good long-term fit — plain-text `%%schedule` cells, and confirming the
  TextMate bridge works for `%%alpha`, are both first-implementation tasks, not blocking design
  questions.
- **A dedicated Alpha Jupyter kernel** (§5.3, §11) — revisit once real usage shows whether the
  `%%alpha`/`%%schedule` magic split on a standard Python kernel feels natural or like friction.
