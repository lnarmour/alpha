# Alpha Linear Variables Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add general linear and unrestricted Alpha variables with exact pointwise polyhedral resource checking, independent of element types and scheduling.

**Architecture:** Parse `linear` as a declaration modifier and resolve it to a per-system `VariableId` plus `Multiplicity`. A new `alpha-model::multiplicity` phase runs after domain inference, computes source-located affine use relations, checks expression-flow compatibility, and proves injectivity, disjointness, and exact coverage. Lowered IR carries multiplicity as metadata; scheduling continues to use existing dependence edges.

**Tech Stack:** Rust, rowan typed CST, logos lexer, isl set/map algebra, Cargo tests.

**Spec:** `docs/superpowers/specs/2026-08-28-alpha-linear-variables-design.md`

## Global Constraints

- Existing declarations are `Unrestricted` by default and retain existing behavior.
- Multiplicity is independent of scalar or quantum types.
- A linear expression cannot flow into an unrestricted binding or unrestricted operator port.
- Every point of every linear declaration must have one exact consumer; output export is a consumer.
- Unsupported linear expression forms emit a diagnostic and are never silently skipped.
- Linearity adds no schedule ordering constraints beyond existing data dependences.
- Work directly on the current branch in the current workspace; do not create a worktree or use subagents.

---

### Task 1: Declaration Syntax And Resolved Representation

**Files:**
- Modify: `alpha-syntax/src/token_kind.rs`
- Modify: `alpha-syntax/src/syntax_kind.rs`
- Modify: `alpha-syntax/src/parser/items.rs`
- Modify: `alpha-syntax/src/ast.rs`
- Create: `alpha-syntax/tests/linear_variables.rs`
- Create: `alpha-model/src/multiplicity.rs`
- Modify: `alpha-model/src/lib.rs`
- Modify: `alpha-model/src/resolve.rs`
- Modify: `alpha-transform/src/ir.rs`
- Modify: `alpha-transform/src/lower.rs`
- Create: `alpha-transform/tests/linear_variables.rs`

**Interfaces:**
- Produces: `Multiplicity::{Linear, Unrestricted}`, `VariableId`, `Resolver::variable_id`, `Resolver::variable_multiplicity`, and `ir::Variable::multiplicity`.
- Consumes: existing variable comma-group and domain inheritance.

- [x] **Step 1: Write parser tests** for `linear X, Y : [N]`, default unrestricted syntax, lossless round-trip, and rejection of `X, linear Y : [N]`.

```rust
#[test]
fn linear_modifier_marks_the_first_variable_in_a_group() {
    let parse = alpha_syntax::parse(SRC);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let vars: Vec<_> = parse.tree().systems().next().unwrap().inputs().unwrap().variables().collect();
    assert!(vars[0].is_linear());
    assert!(!vars[1].is_linear());
}
```

- [x] **Step 2: Run `cargo test -p alpha-syntax --test linear_variables`** and confirm it fails because `linear` and `is_linear` are absent.
- [x] **Step 3: Add `KwLinear`/`KW_LINEAR`, teach clause lookahead to skip the prefix, consume it inside the first variable node, and expose `Variable::is_linear()`**. Keep `AFFINE_FUZZY_VARIABLE_USE` as the final `SyntaxKind`.
- [x] **Step 4: Run `cargo test -p alpha-syntax --test linear_variables` and `cargo test -p alpha-syntax`**.
- [x] **Step 5: Write lowering tests** asserting a linear comma group lowers both variables as linear while an ordinary declaration lowers unrestricted.
- [x] **Step 6: Run `cargo test -p alpha-transform --test linear_variables`** and confirm it fails because multiplicity metadata is absent.
- [x] **Step 7: Add the semantic representation**:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Multiplicity { Linear, #[default] Unrestricted }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VariableId(u32);
```

Assign IDs in declaration source order. Resolve group multiplicity by walking backward from each domain-bearing terminator through its preceding bare-name siblings until the previous terminator; the group is linear exactly when its first node has `KW_LINEAR`.
- [x] **Step 8: Add `multiplicity` to `ir::Variable`, populate it in lowering, update all IR constructors, and run `cargo test -p alpha-transform --test linear_variables`**.
- [x] **Step 9: Run `cargo test -p alpha-model -p alpha-transform`**.
- [ ] **Step 10: Commit with `git commit -m "feat: represent linear variable declarations"`**.

### Task 2: Structural Multiplicity And Assignment Checking

**Files:**
- Modify: `alpha-model/src/multiplicity.rs`
- Modify: `alpha-model/src/diagnostic.rs`
- Modify: `alpha-model/src/analyze.rs`
- Create: `alpha-model/tests/multiplicity.rs`

**Interfaces:**
- Consumes: `VariableId`, `Multiplicity`, resolver domains, expression/context domains.
- Produces: `check_system(resolver, system, domains, contexts) -> Vec<Diagnostic>`, `ExprFacts`, and source-located `ResourceUse` records.

- [x] **Step 1: Write failing model tests** for linear identity transfer, unrestricted-to-linear restriction, linear-to-unrestricted rejection, and linear operands passed to existing unary/binary/multi-arg operators.

```rust
#[test]
fn linear_value_cannot_flow_to_unrestricted_target() {
    let diagnostics = check(SRC_LINEAR_TO_UNRESTRICTED);
    assert!(diagnostics.iter().any(|d| matches!(d, Diagnostic::LinearValueWidened { .. })));
}
```

- [x] **Step 2: Run `cargo test -p alpha-model --test multiplicity`** and confirm the new diagnostic/API is absent.
- [x] **Step 3: Add `LinearValueWidened`, `LinearArgumentToUnrestrictedPort`, and `LinearityUnsupportedHere` diagnostics** with source ranges and stable messages.
- [x] **Step 4: Implement expression inference**. Literals/index values are unrestricted; variable references use resolved multiplicity; dependence/restriction/auto/paren preserve result multiplicity; existing operators have all-unrestricted signatures and reject linear operands; unsupported index-changing forms report `LinearityUnsupportedHere` only when they contain a linear reference.
- [x] **Step 5: Check standard equations** using the explicit four-case assignment compatibility table from the spec and call the pass from `analyze_system` after domain inference.
- [x] **Step 6: Run `cargo test -p alpha-model --test multiplicity` and `cargo test -p alpha-model`**.
- [ ] **Step 7: Commit with `git commit -m "feat: check linear value flow"`**.

### Task 3: Exact Polyhedral Consumer Accounting

**Files:**
- Modify: `alpha-model/src/multiplicity.rs`
- Modify: `alpha-model/src/diagnostic.rs`
- Modify: `alpha-model/tests/multiplicity.rs`

**Interfaces:**
- Consumes: structural `ExprFacts` and exact expression/context domains.
- Produces: exact `ResourceUse { variable, relation, start, end }` collection and resource diagnostics.

- [x] **Step 1: Add failing tests** for one identity use, a broadcast/non-injective access, two overlapping reads, partial coverage, and output-boundary consumption.
- [x] **Step 2: Run the focused tests** and confirm accepted invalid programs or missing diagnostics.
- [x] **Step 3: Derive each variable access map** by composing the enclosing consumer domain with nested dependence functions and restrictions. Bare references use the identity map on their validated context.
- [x] **Step 4: Add `LinearUseNotInjective`, `LinearUsesOverlap`, and `LinearValueUnconsumed` diagnostics**. For each variable, use exact ISL `is_injective`, range intersection, union, and domain subtraction; render the exact offending set in `detail`.
- [x] **Step 5: Treat a linear output boundary as one full-domain consumer and suppress coverage cascades for variables already carrying `LinearityUnsupportedHere`**.
- [x] **Step 6: Run `cargo test -p alpha-model --test multiplicity` and `cargo test -p alpha-model`**.
- [ ] **Step 7: Commit with `git commit -m "feat: check exact linear resource use"`**.

### Task 4: Branch-Sensitive Resource Summaries

**Files:**
- Modify: `alpha-model/src/multiplicity.rs`
- Modify: `alpha-model/src/diagnostic.rs`
- Modify: `alpha-model/tests/multiplicity.rs`

**Interfaces:**
- Consumes: `LinearUses` relation summaries.
- Produces: domain-union semantics for `case` and alternative equality semantics for runtime `if`.

- [x] **Step 1: Add failing tests** for disjoint case reads passing, overlapping case reads failing through existing completeness diagnostics, equal runtime-if summaries passing, and differing branch variables/maps failing.
- [x] **Step 2: Run the focused branch tests** and confirm the expected failures.
- [x] **Step 3: Implement case combination** by retaining each branch's inferred context restriction and unioning all reachable branch summaries.
- [x] **Step 4: Add `LinearBranchMismatch` and implement runtime-if comparison** using exact ISL map equality per `VariableId`; ignore statically empty branch contexts and count an equal branch summary once.
- [x] **Step 5: Count condition uses in addition to the selected branch summary and rerun `cargo test -p alpha-model --test multiplicity`**.
- [x] **Step 6: Run `cargo test -p alpha-model`**.
- [x] **Step 7: Commit with `git commit -m "feat: analyze linear control flow"`**.

### Task 5: System Calls, External Signatures, And Producer Accounting

**Files:**
- Modify: `alpha-syntax/src/parser/items.rs`
- Modify: `alpha-syntax/src/ast.rs`
- Modify: `alpha-model/src/multiplicity.rs`
- Modify: `alpha-model/src/diagnostic.rs`
- Modify: `alpha-model/src/analyze.rs`
- Modify: `alpha-model/tests/multiplicity.rs`
- Modify: `alpha-syntax/tests/linear_variables.rs`

**Interfaces:**
- Produces: `PortSignature { inputs, outputs }`, explicit external signature syntax, system port checks, and exact linear definition relations.

- [x] **Step 1: Add parser tests** for `external move(linear) -> linear`, `external observe(linear) -> unrestricted`, `external destroy(linear) -> ()`, and unchanged `external f(2)`.
- [x] **Step 2: Run the parser test and confirm signature syntax fails**.
- [x] **Step 3: Parse explicit port lists** into typed AST accessors while retaining integer cardinality. Resolve legacy cardinality to all-unrestricted inputs and one unrestricted output.
- [x] **Step 4: Add failing semantic tests** for matching system-call ports, linear arguments to unrestricted ports, multiple linear outputs, and incomplete linear use-equation production.
- [x] **Step 5: Implement `PortSignature` resolution** for built-ins, externals, and system declarations. Require linear use-equation output positions to reduce to affine dependence accesses over declared linear variables.
- [x] **Step 6: Add `LinearDefinitionIncomplete` and check definition maps** for injectivity, pairwise disjointness, and full target-domain coverage.
- [x] **Step 7: Run `cargo test -p alpha-syntax -p alpha-model`**.
- [x] **Step 8: Commit with `git commit -m "feat: add linear port signatures"`**.

### Task 6: Regression And Schedule Independence

**Files:**
- Modify: `alpha-transform/src/normalize_reduction.rs` if generated locals need explicit unrestricted metadata.
- Modify: IR constructors in tests and codegen as identified by compilation.
- Create or modify: `alpha-codegen/tests/linear_schedule.rs`

**Interfaces:**
- Consumes: completed multiplicity metadata and analysis.
- Produces: workspace-wide compatibility and evidence that schedules do not alter resource results.

- [ ] **Step 1: Run `cargo test --workspace --no-fail-fast`** and fix only compilation/regression failures caused by the new required IR field or syntax keyword.
- [ ] **Step 2: Add a schedule regression** that checks two different legal schedules for the same linear transfer system validate successfully after the source passes multiplicity analysis.
- [ ] **Step 3: Run the focused codegen schedule test**.
- [ ] **Step 4: Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace --no-fail-fast`**.
- [ ] **Step 5: Confirm `git diff --check` and review `git status --short` for unrelated changes**.
- [ ] **Step 6: Commit with `git commit -m "test: cover linear schedule independence"`**.
