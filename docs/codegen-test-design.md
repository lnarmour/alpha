# `alpha-codegen` test design: snapshot coverage for scheduled codegen

Status: draft, for iteration. Companion to `docs/scheduled-codegen-design.md` (the "§" references
below point there unless stated otherwise).

## 1. Goal and scope

`alpha-codegen` currently has plain `assert!`/`assert_eq!` unit tests scattered across
`stmt.rs` (2), `schedule.rs` (5), `legality.rs` (5), `scheduledc.rs` (3), `describe.rs` (3), plus
two integration-level files: `tests/codegen_fixtures.rs` (a whole-corpus smoke test that only
exercises `WriteC`/`generate_system`, never `ScheduledC`) and `tests/scheduledc_e2e.rs` (compiles
and runs generated C for exactly one fixture, `PrefixSum`, cross-checked against `WriteC`). No
`insta`/snapshot testing exists anywhere in the workspace today.

This doc plans a denser, snapshot-based test suite for `alpha-codegen` — primarily exercising
`ScheduledC` (the new backend: `stmt.rs`/`schedule.rs`/`legality.rs`/`scheduledc.rs`) since that's
where schedule-driven behavior (loop fusion/splitting, reordering, reduce splitting) actually lives,
but reusing `expr.rs`'s shared expression codegen means most of this coverage benefits `WriteC` too.
Not in scope: performance, Barvinok-feature-gated sizing, the `alpha-py`/notebook layer (already has
its own `pytest`/`nbval` suite), or re-testing isl itself (`isl/tests/smoke.rs` already covers the
lex-order/width finding this design leans on).

## 2. Why snapshots, and what to snapshot

Hand-written `assert!(code.contains("Y"))`-style assertions (the current style in `scheduledc.rs`'s
own tests) prove generation didn't error and mention a name, but say nothing about *shape* — loop
nesting, fusion, ternary-chain structure, combine-table text. A snapshot pins the exact rendered
text so a future change to, say, `gen_case`'s ternary construction or the AST walker's loop-emission
order shows up as a reviewable diff instead of silently passing a weak substring check. This is a
good fit here specifically because the design doc already establishes (§12 step 8) that generated
output has **no timestamps/addresses/nondeterminism** — a diff on re-run means a real behavior
change, not fixture flakiness. That property is what makes snapshotting viable at all.

The open design question isn't "snapshots: yes or no" but **what granularity to snapshot at**, since
`ScheduledC` builds the *entire program* as one `AstBuild::generate` call (§8.1) — there's no
built-in notion of "just this statement's generated code" the way `WriteC`'s one-function-per-
variable model would have made trivial. Four tiers, from finest to coarsest:

| Tier | What's snapshotted | Where it lives | Needs a schedule/AST walk? |
|---|---|---|---|
| 1. Expression fragment | One `CExpr`'s `Display` text for one AST node shape | white-box, in `expr.rs`'s own test module | No — calls `gen_value` directly |
| 2. Statement body | One statement's full `Vec<Stmt>` (prologue + assignment/combine) | white-box, in `scheduledc.rs`'s own test module | No — calls `gen_statement_body` directly with hand-picked index names |
| 3. Driver body (loop nest) | The AST-walked loop/statement tree for a whole (small) program under a given schedule | black-box, `tests/*.rs`, public API only | **Yes** — this is the only tier where loop structure exists at all |
| 4. Schedule skeleton | `describe_system`/`describe_normalized_system`'s ISL-text repr | black-box, `tests/*.rs` or extending `describe.rs`'s own tests | No — no legality check, no codegen |
| 5. Diagnostic | A rejected schedule's/illegal-schedule's error `Display` text | white-box (in `schedule.rs`/`legality.rs`) and black-box (in `tests/*.rs` for cross-statement cases) | Only as far as validation, never codegen |

This directly answers the prompt's "case expression inside a reduction body → snapshot the loops
emitted" example: that's inherently a **Tier 3** case, not Tier 1/2, because the loop headers
(`for i`, `for j`) only come into existence after the *whole-program* schedule is built and walked
(§8.2) — a single statement's own body codegen (`gen_statement_body`) never sees a loop, only the
ternary/assignment it's wrapped in. Tiers 1–2 are still valuable (cheap, no isl AST build needed,
pinpoint diffs for expression-codegen regressions), just answering a narrower question than "what do
the loops look like."

## 3. Harness changes needed

- **`alpha-codegen/Cargo.toml`**: add `insta = "1"` under `[dev-dependencies]`. No feature flags
  needed for plain string snapshots (no YAML/JSON/redaction machinery required, given point 2's
  determinism property). `cargo-insta` (the review CLI, `cargo install cargo-insta`) is a developer
  tool, not a crate dependency — worth a one-line mention in `alpha-codegen`'s own doc comment or
  `docs/design.md` for discoverability, not a hard requirement.
- **File snapshots throughout.** Every tier (1/2/4/5 as well as 3) uses `insta::assert_snapshot!(value)`
  with no inline `@"..."` — one `.snap` file per test, under `src/snapshots/` (for the white-box
  tests embedded in `expr.rs`/`scheduledc.rs`/`schedule.rs`/`legality.rs`/`describe.rs`) or
  `tests/snapshots/` (for the black-box tests in `tests/scheduledc_snapshots.rs`). Uniform workflow:
  `cargo insta review` accepts/rejects every pending snapshot the same way regardless of tier, rather
  than needing inline-vs-file judgment calls per test.
- **`test_util.rs` additions**:
  - A minimal test-double `ExprGen` (call it `TestGen`) for Tier 1, so `expr.rs`'s own tests don't
    need to reach into `scheduledc::Gen` (private to that module) or `writec::Gen`. Just enough to
    resolve a handful of input/local variable names to `VarInfo`, a no-op `add_extern`, `render_read`
    as `name(idx...)`, and a `gen_reduce_value` that either delegates to `gen_value` on the reduce's
    own body (to test the "`Reduce` nested directly in another `Reduce`'s body" documented
    unsupported case, §7.3/§11) or returns the same "unreachable" error `scheduledc::Gen` does.
  - More small inline fixture constants alongside the existing `PREFIX_SUM`/`PLAIN_COPY`/
    `PREFIX_SUM_WITH_CONSUMER`, one per matrix row in §5 below that needs its own system (most rows
    are a 3–6 line `.alpha` string, matching the existing style).
- **`scheduledc.rs` test module**: a tiny helper to render a `Vec<Stmt>` to text for Tier 2, since
  `simplec::Stmt` has no standalone `Display` (only `simplec::Function` does). Cheapest option:
  wrap the statements in a throwaway `Function { name: "test", params: vec![], .. }` and use its
  existing `Display` — no changes to `simplec.rs`'s public shape needed.
- **New `tests/scheduledc_snapshots.rs`**: Tier 3/4, black-box, using only `generate_scheduled_system`
  / `generate_system` / `describe_normalized_system`. A helper `extract_driver_body(code: &str) ->
  &str` slices between the two marker comments `scheduledc.rs::build_driver` already emits verbatim
  — `"// Evaluate every statement, in schedule order."` through (not including) `"// Free all
  allocated memory."` — which captures exactly the collected loop-iterator declarations plus the
  AST-walked statement tree, and nothing else (no `#include`s, macros, or storage-allocation
  boilerplate to cause unrelated churn). This keeps Tier 3 snapshots small and focused on the thing
  actually being tested — loop/statement shape — while a couple of full-file snapshots (§5.9) stay
  as a coarser end-to-end sanity net.

## 4. A note on RHS syntax used below

Every snippet below is either a real, already-checked-in fixture (cited by path) or built from a
grammar rule confirmed against a real fixture using the same construct. The two constructs with no
exact real-world fixture precedent — a `min`/`max` **multi-arg** (non-reduce) call, and `reduce`
with an external combiner — were spiked end-to-end (parse → `analyze_system` → `lower_system` →
normalize → `WriteC` generate) against a scratch source file before writing this doc, not just
parse-checked: `min(X[i], X[i])` lowers to `ir::ExprKind::MultiArg { operator:
Operator::Named("min"), .. }`, and `reduce(delta, [j], {:j<=i}: X[j])` (with `external delta(2)`
declared) lowers to `ir::ExprKind::Reduce { operator: Operator::External("delta"), .. }` — both
generate cleanly through the existing `WriteC` backend. Both rows in §5.1/§5.4 are confirmed valid
as written.

## 5. The test matrix

### 5.1 Single 1D output, identity/default schedule — RHS node-kind ladder (Tier 1, occasional Tier 3)

One system per row (or grouped where trivial), `outputs Y: [N]`, `inputs X: [N]` unless noted.
Increasing complexity, each isolating one `ir::ExprKind` arm:

| # | RHS shape | Snippet | Node(s) exercised |
|---|---|---|---|
| 1 | Int/Real literal | `Y[i] = 3;` / `Y[i] = 3.0;` | `Int`, `Real` |
| 2 | Dependence read | `Y[i] = X[i];` (existing `PLAIN_COPY`) | `Dependence`/`Variable` |
| 3 | Binary op | `Y[i] = X[i] + X[i];` | `Binary` |
| 4 | Unary op | `Y[i] = -X[i];` | `Unary` |
| 5 | Index function (affine, no array read) | `Y[i] = val (i->N-i);` (real form, `.../syntax-tests/index1.alpha`) | `IndexFunction` — nice pairing with the `i->N-i` schedule flavor from the prompt |
| 6 | If/then/else | `Y[i] = if (X[i] > 0) then X[i] else -X[i];` (real form, `.../normalize-tests/dependence.alpha:118`) | `If` |
| 7 | Case, 2 branches | `Y[i] = case { {:i<N/2}: X[i]; auto: -X[i]; };` (shape from `.../syntax-tests/autoRestrict1.alpha`) | `Case`, `AutoRestrict` |
| 8 | Case, 3 branches | adds a third guarded branch to #7 | `Case` (deeper ternary chain) |
| 9 | N-ary `sum` | `Y[i] = sum(X[i], X[i], 1[]);` (real form, `.../distributivity1.alpha:64`) | `MultiArg` + `Operator::Named("sum")` |
| 10 | External call | `external sqrt(1)` / `Y[i] = sqrt(X[i]);` (real form, `.../kernels/cholesky.alpha`) | `MultiArg` + `Operator::External` |
| 11 | N-ary `min`/`max` (not reduce) | `Y[i] = min(X[i], X[i]);` — constructed, confirmed (§4) to lower as `MultiArg` + `Operator::Named("min")` | `MultiArg` + `Operator::Named("min")` |

Rows 1–6, 9–11 are pure Tier 1 (`gen_value` → `CExpr::Display`, no domains/loops needed). Rows 7–8
need a real `context_domain` on each branch to render the ternary's condition text (`gen_case` calls
`ambient_build`/`expr_from_set`), so they need the full parse→normalize pipeline (already what
`test_util::normalized_system` gives you) even though still no schedule/AST-build. Convolution/
Select/IndexPolynomial/`argreduce` are deliberate non-goals (§11) — no positive coverage needed here,
just the negative check in §5.10.

### 5.2 Same ladder, explicit reversing schedule `i -> N-1-i` (Tier 3)

Pick 2–3 representative rows from 5.1 (literal, dependence, if/then/else are enough — the RHS
codegen path is identical regardless of schedule, so this isn't testing 11 more combinations, just
confirming schedule and expression codegen are properly decoupled). For each, generate with
`schedule_text = "{ Y[i] -> [N-1-i]; }"` and snapshot the extracted driver body. Expect: the `for`
loop's init/cond/inc flip direction; the statement body text is byte-identical to the identity-
schedule version. A test that asserts *that* equality directly (not just two separate snapshots) is
worth adding — it's a stronger, more direct claim than "both snapshots happen to look right."

Legality note: every row here is an ordinary (non-reduce) statement reading only inputs, so any
bijective reschedule is automatically legal (no producer/consumer edge to violate) — a useful
explicit contrast to §5.4/§5.6, where reordering *does* matter.

### 5.3 A third schedule flavor: reversing the *inner* reduce dimension (Tier 3)

Using `PREFIX_SUM` (`Y[i] = reduce(+, [j], {:j<=i}: X[j])`), compare:
- ascending: `{ Y__init[i]->[i,0,0]; Y__reduce[i,j]->[i,1,j]; }` (already `test_util`'s canonical
  schedule)
- descending: `{ Y__init[i]->[i,0,0]; Y__reduce[i,j]->[i,1,N-1-j]; }`

Both legal (the RAW edge is only on `i`, §4.2 — `j`-order is unconstrained by it), both should
compile/run to the same numeric result (worth a `scheduledc_e2e.rs`-style compile+run check
alongside the snapshot, not just a snapshot — this is exactly the kind of thing that "looks right"
in generated C but silently computes wrong answers if `j`'s bound flip interacts badly with the
init/accumulate split). The snapshot pair makes the inner-loop reversal visible; the numeric check
makes sure it's still correct.

### 5.4 Reduction body node-kind ladder (Tier 2 for operator/combine-table shape, Tier 3 for the case-body flagship)

| # | Variant | Snippet basis | What it tests |
|---|---|---|---|
| 1 | `+` (baseline) | `PREFIX_SUM` (existing) | already covered |
| 2 | `*` | swap operator, cf. `.../hoistOutOfReduction2.alpha` | neutral element `1.0f`, `*` combine |
| 3 | `min` | cf. `.../reuse-analysis/rnaEnergy.alpha` (real `min` reduce) | neutral element `INFINITY`, `min(...)` combine |
| 4 | `max` | cf. `.../basic/CNN.alpha` (real `max` reduce) | neutral element `-INFINITY`, `max(...)` combine |
| 5 | external combiner | `external delta(2)` / `Y[i] = reduce(delta, [j], {:j<=i}: X[j]);` — constructed, confirmed (§4) to lower as `Reduce` + `Operator::External("delta")` | `Operator::External` combine path, `add_extern` |
| 6 | **case inside the reduce body** (the prompt's flagship example) | `Y[i] = reduce(+, [j], {:j<=i}: case { {:j=i}: X[j]*2[]; auto: X[j]; });` | **Tier 3**: the ternary from `gen_case` living inside the `for j` loop nested inside `for i` — this is the one row in this whole doc that most directly answers "snapshot the loops emitted as part of the reduction body" |
| 7 | reduce body is a `sum`/binary expression | e.g. `reduce(+, [j], {:j<=i}: X[j] + X[j])` | confirms `gen_value`'s shared path behaves identically whether reached from an `Ordinary` statement (§5.1) or a `ReduceStep` (§4.2) — cheap regression net for accidental divergence between the two call sites |

Rows 1–5, 7 are Tier 2 (`gen_statement_body` on the `ReduceInit`/`ReduceStep` `Statement`, no AST
walk) — the neutral element and combine-table text don't depend on loop structure at all. Row 6 is
the one exception, per the tier-3 rationale in §2.

### 5.5 Two 1D outputs — dataflow × schedule combos (mixed tiers, mostly Tier 3/4)

| # | Dataflow shape | Snippet basis | Schedule variants to snapshot |
|---|---|---|---|
| 1 | Independent, no shared data | `Y[i]=X[i];` / `W[i]=-X[i];` (two inputs or one shared input, either way no dependence edge between them) | (a) both default/identity — legal; (b) **fused**, same loop level: `{ Y[i]->[i,0]; W[i]->[i,1]; }`; (c) **separate**, sequential: `{ Y[i]->[0,i]; W[i]->[1,i]; }`. (b) vs (c) is the concrete "fusing" example from the prompt — snapshot both to show one interleaved loop vs. two sequential loops over the same `i` range |
| 2 | Producer → consumer, no reduce | `Y[i]=X[i];` then `W[i]=Y[i]+1[];` | legal sequential (Y before W) — Tier 3; reversed (W before Y) — **illegal**, Tier 5 (black-box diagnostic snapshot, since this is a cross-statement case not covered by `legality.rs`'s existing single-reduce unit tests) |
| 3 | Producer → consumer through a reduce | existing `PREFIX_SUM_WITH_CONSUMER` (`Y[i]=reduce(+,...)`, `Z[i]=Y[i]+1[]`) | fully-legal explicit schedule — Tier 3 (this is also the natural place for a full-file Tier-3 snapshot, §5.9, since it's small but exercises the reduce-pair + ordinary-consumer shape together); illegal (Z before Y completes) is already unit-tested in `legality.rs::consumer_scheduled_before_its_producer_is_illegal` — propose upgrading that existing test to an inline snapshot rather than adding a new one |
| 4 | Two independent reduces, fan-out from one input | `Y[i]=reduce(+,...)` and `MaxY[i]=reduce(max,...)`, both over `X` | confirms 4 independently-schedulable statements (`Y__init`/`Y__reduce`/`MaxY__init`/`MaxY__reduce`) don't spuriously depend on each other — Tier 4 (`describe_normalized_system` enumerates all 4 with correct domains) plus one Tier 3 fused-vs-separate pair, same spirit as row 1 |
| 5 | Nested reduce dependency (richest case) | `Y[i]=reduce(+,[j],{:j<=i}:X[j]);` then `Z[i]=reduce(+,[j],{:j<=i}:Y[j]);` (prefix-sum-of-prefix-sum) | the real stress test for `legality.rs`: `Y`'s *entire* accumulation for a given `i` must finish before any `Z__reduce` instance reads `Y[j]` for `j<=i` — a strictly stronger constraint than row 3's single-reduce case. One legal fully-sequential schedule (Tier 3, 4-statement loop nest) and at least one illegal interleaving (Tier 5) |

### 5.6 Piecewise / multi-`SystemBody` equations for one variable (design decision 2, §2)

Real attested shape (`.../recursive-subsystems/strassen.alpha`): `when {domain} let ... let ...`
gives one guarded body plus an implicit-complement body, both defining the same output. A small
adapted example:

```
when {:i=0} let
    Y[i] = X[i];
let
    Y[i] = X[i] + Y[i-1];
```

(exact legality of the second branch — a genuine recurrence — needs checking at implementation
time; the point of this row is purely the piecewise-merging behavior, not a new interesting
dependence shape, so simplify to something legality-trivial if the recurrence complicates things,
e.g. both branches read only `X`). Confirms (a) `stmt::statements` keeps this as **one** statement
(`Y`, `Ordinary` with 2 equations) rather than splitting per-body (Tier — direct `stmt::statements`
unit test, no snapshot needed, `stmt.rs` already has the harness for this); (b) `gen_statement_body`
renders the two-branch ternary chain correctly (Tier 2); (c) `describe_normalized_system` shows
exactly one `Y[...]` entry despite 2 source equations (Tier 4).

### 5.7 Schedule-skeleton (Tier 4) coverage, standalone

`describe.rs` already has 3 tests (pre-normalization no-split, post-normalization split, reflects an
explicit schedule). Extend with: the two-output fan-out case (5.5 row 4, confirming 4-statement
enumeration), and the piecewise case (5.6). These are cheap (no legality check, no codegen) and give
a fast, readable regression net for statement naming/domain-shape independent of whether a given
schedule is even legal — useful as a first thing to check when a matrix row's Tier 3 snapshot
breaks, to localize whether the break is in statement enumeration or in codegen/legality.

### 5.8 Diagnostic snapshot upgrades (Tier 5)

Upgrade existing `assert!(err.contains(...))` tests to `insta::assert_snapshot!` on the full error
`Display` text, keeping a short `contains` assertion alongside each (matches the existing style,
keeps test intent visible without opening a snapshot file):

- `schedule.rs`: `unknown_statement_name_is_rejected`, `partially_specified_mentioned_statement_is_rejected`,
  `non_injective_mentioned_statement_is_rejected`, `mismatched_explicit_widths_are_rejected` (4)
- `legality.rs`: `identity_default_schedule_is_illegal_for_a_real_reduce_dependency`,
  `consumer_scheduled_before_its_producer_is_illegal`, `reduce_scheduled_before_its_own_init_is_illegal` (3)
- `scheduledc.rs`: `empty_schedule_text_is_rejected_as_illegal_for_prefix_sum` (1)

Plus new black-box diagnostics for the two-output cases that don't have a unit-level equivalent
today (§5.5 rows 2 and 5's illegal variants).

### 5.9 A couple of full-file Tier 3 snapshots, as a coarser sanity net

Alongside the extracted-driver-body slices (which deliberately drop preamble/storage/macro
boilerplate to stay focused), keep 1–2 whole-generated-C snapshots — `PrefixSum` is the natural
choice, already the subject of `scheduledc_e2e.rs`'s numeric check — so a change to the preamble
(includes, macros, storage layout) that doesn't affect any driver-body slice still gets caught
somewhere. Low volume on purpose: these are the snapshots most prone to unrelated-change churn.

### 5.10 ScheduledC-side scope-boundary corpus check — in scope for this pass

`tests/codegen_fixtures.rs`'s whole-corpus smoke test only ever calls `generate_system` (`WriteC`) —
`generate_scheduled_system` is never run against the bundled `.alpha` fixture corpus at all. Add a
parallel test (same fixture-walking logic, `ScheduledC` instead, empty schedule text — a fixture
that legitimately needs a real explicit schedule to pass legality, e.g. anything with a top-level
reduce, is expected to fail with `CodegenError::IllegalSchedule`, which this test should treat the
same way it treats a known `Unsupported` scope boundary: a recognized, non-bug outcome, not a
failure — only an *unexpected* error variant, or an unexpected error message, should fail the test)
to catch the same class of "unexpected `Unsupported`/internal-error" regressions `WriteC`'s corpus
test already catches, just for the newer backend. Not a snapshot test — plain pass/fail like its
`WriteC` counterpart — but confirmed in scope for this pass rather than deferred.

## 6. File/module layout summary

```
alpha-codegen/
  Cargo.toml                        + insta = "1" under [dev-dependencies]
  src/
    test_util.rs                    + TestGen (Tier 1 double), + new fixture constants per §5 row
    expr.rs                         + #[cfg(test)] mod tests (new) — §5.1 Tier 1
    scheduledc.rs                   extend existing tests — §5.4 Tier 2, §5.8 (own diagnostic)
    schedule.rs                     extend existing tests — §5.8 (4 diagnostics)
    legality.rs                     extend existing tests — §5.8 (3 diagnostics)
    describe.rs                     extend existing tests — §5.6/§5.7 Tier 4
    stmt.rs                         + one more unit test — §5.6(a), no snapshot
  tests/
    scheduledc_snapshots.rs (new)   §5.2, §5.3, §5.4 row 6, §5.5, §5.9 — Tier 3/4, black-box
    codegen_fixtures.rs             + a ScheduledC-side scope-boundary test — §5.10
    snapshots/                      (insta file-snapshot storage for the above)
```

## 7. Suggested phasing

1. Harness only: `insta` dev-dependency, `TestGen`, `extract_driver_body` helper, the `Vec<Stmt>` →
   `Function` rendering trick — prove the mechanics work with one or two throwaway tests before
   committing to the full matrix.
2. §5.1 (Tier 1 ladder) — cheapest, no isl AST build, proves the harness end to end across every
   `ExprKind` arm.
3. §5.2/§5.3/§5.4 (single-output schedule variation + reduce operator/case-body ladder, including
   the flagship case-in-reduce-body example).
4. §5.7/§5.8 (Tier 4 skeletons + Tier 5 diagnostic upgrades) — mostly mechanical, low risk.
5. §5.5 (two-output matrix) — richest and most novel, saved for last once conventions are proven on
   the simpler rows.
6. §5.10 (ScheduledC-side scope-boundary corpus check) — independent of the snapshot matrix, can
   land any time; grouped last here only because it's a separate file/concern from the rest of this
   phasing, not because it's lower-priority.
7. Stretch/optional: §5.6 (piecewise).

## 8. Decisions (resolved)

1. **File snapshots throughout** — no inline `@"..."` anywhere, uniform `cargo insta review` workflow
   across all five tiers. Reflected in §3/§6.
2. **Grammar assumptions confirmed, not just planned.** Both previously-flagged rows (§5.1 row 11's
   non-reduce `min`/`max` multi-arg; §5.4 row 5's external reduce combiner) were spiked end-to-end
   before finalizing this doc — see §4 — and are in the plan as confirmed, not tentative.
3. **§5.10 (ScheduledC-side scope-boundary corpus check) is in scope for this pass**, not deferred —
   see §7 step 6.
4. **Phasing order confirmed as proposed** (§7) — no reordering; two-output cases (§5.5) stay last,
   after the simpler tiers have proven out the harness conventions.

Plan is settled; ready to move into implementation starting with §7 step 1 (harness only).
