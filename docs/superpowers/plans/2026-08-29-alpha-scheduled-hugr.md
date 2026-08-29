# Alpha Scheduled HUGR Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add explicitly linear qubit values, typed quantum call equations, compact trajectory-based realization, and scheduled HUGR generation to Alpha.

**Architecture:** Extend the existing semantic and transform IRs with element types and resolved operation calls. Reuse ISL for schedule validation and AST generation, immediately translate the ISL AST into an Alpha-owned scheduled IR, and lower that IR independently to C or HUGR. HUGR generation specializes symbolic parameters, maps logical qubit trajectories to rectangular borrow arrays, proves occupancy safety, and emits structured `TailLoop`/`Conditional` dataflow.

**Tech Stack:** Rust 2021, rowan/logos parser, ISL and local safe wrappers, HUGR 0.29.3, TKET 0.21.2, PyO3 0.23, pytest, insta snapshots.

**Spec:** `docs/superpowers/specs/2026-08-29-alpha-scheduled-hugr-design.md`

## Global Constraints

- Work on `louis/hugr`; do not create a worktree unless explicitly requested.
- Keep multiplicity separate from element type. A `qubit` binding is valid only when explicitly marked `linear`.
- Keep logical resource flow schedule-independent. A schedule may order flow edges but cannot create or redirect them.
- Use ISL for polyhedral scanning. Do not implement a replacement for ISL/CLooG code generation.
- Translate ISL AST expressions to typed Rust enums before either backend; HUGR must never parse C text.
- Preserve the immutable `System -> NormalizedSystem -> ScheduledSystem` Python pipeline.
- HUGR generation requires concrete bindings for every shape- or control-affecting Alpha parameter.
- The first realization accepts rectangular resource groups and affine projection/permutation lane maps only.
- Never fall back to allocating one qubit per logical-domain point when compact realization fails.
- Use `collections.borrow_arr` for partially occupied linear collections and prove every unsafe borrow-array precondition statically.
- `measure` consumes its qubit. Runtime measurement-dependent gate control is rejected explicitly in this increment.
- Do not add source `inouts`; use explicit linear inputs and outputs.
- Do not add cross-trajectory lane reuse, irregular packing, parametric HUGR emission, or imported HUGR extension metadata in this plan.
- Preserve all existing untyped Alpha programs, scheduled-C output, and linearity diagnostics.
- Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace --no-fail-fast` before completion.

---

### Task 1: Parse First-Class Element Types

**Files:**
- Modify: `alpha-syntax/src/token_kind.rs`
- Modify: `alpha-syntax/src/syntax_kind.rs`
- Modify: `alpha-syntax/src/parser/items.rs`
- Modify: `alpha-syntax/src/ast.rs`
- Create: `alpha-syntax/tests/element_types.rs`

**Interfaces:**
- Produces: `alpha_syntax::ast::ElementType::{Bool, Int, Real, Qubit}`.
- Produces: `Variable::element_type() -> Option<ast::ElementType>` with comma-group inheritance left to the semantic resolver, matching domain and multiplicity inheritance.

- [x] **Step 1: Add parser tests for typed variable groups and lossless round-tripping.**

```rust
use alpha_syntax::ast::ElementType;

#[test]
fn parses_explicit_qubit_type() {
    let source = r#"affine circuit [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 4} of qubit;
    outputs M : {[i] : 0 <= i < 4} of bool;
    let M[i] = false;
.
"#;
    let parse = alpha_syntax::parse(source);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    assert_eq!(parse.syntax_node().text().to_string(), source);
    let system = parse.tree().systems().next().unwrap();
    let input = system.inputs().unwrap().variables().next().unwrap();
    let output = system.outputs().unwrap().variables().next().unwrap();
    assert_eq!(input.element_type(), Some(ElementType::Qubit));
    assert_eq!(output.element_type(), Some(ElementType::Bool));
}

#[test]
fn untyped_variables_remain_valid() {
    let parse = alpha_syntax::parse(
        "affine id [N] -> {:N>0} inputs X:[N] outputs Y:[N] let Y[i]=X[i];.",
    );
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let variable = parse
        .tree().systems().next().unwrap()
        .inputs().unwrap().variables().next().unwrap();
    assert_eq!(variable.element_type(), None);
}
```

Also test `of int`, `of real`, malformed `of`, and `linear A, B : [N] of qubit` where only the terminating declaration owns the CST type token.

- [x] **Step 2: Run the focused syntax test and confirm it fails.**

Run: `cargo test -p alpha-syntax --test element_types`

Expected: compile failure because `ast::ElementType` and `Variable::element_type` do not exist.

- [x] **Step 3: Add type tokens and syntax kinds.**

Add `KwOf`, `KwBool`, `KwInt`, `KwReal`, and `KwQubit` in both token enums and their conversion match. Add `ELEMENT_TYPE` as a node kind.

- [x] **Step 4: Parse an optional type suffix after the terminating variable domain.**

Extend `variable_clause` after the domain/range parse:

```rust
if p.at(T::KwOf) {
    p.start_node(SyntaxKind::ELEMENT_TYPE);
    p.bump();
    if p.at_any(&[T::KwBool, T::KwInt, T::KwReal, T::KwQubit]) {
        p.bump();
    } else {
        p.error("expected 'bool', 'int', 'real', or 'qubit' after 'of'");
    }
    p.finish_node();
}
```

Add the typed AST enum and accessor:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementType { Bool, Int, Real, Qubit }

impl Variable {
    pub fn element_type(&self) -> Option<ElementType> {
        let node = self.0.children().find(|node| node.kind() == K::ELEMENT_TYPE)?;
        node.children_with_tokens()
            .filter_map(|element| element.into_token())
            .find_map(|token| match token.kind() {
                K::KW_BOOL => Some(ElementType::Bool),
                K::KW_INT => Some(ElementType::Int),
                K::KW_REAL => Some(ElementType::Real),
                K::KW_QUBIT => Some(ElementType::Qubit),
                _ => None,
            })
    }
}
```

- [x] **Step 5: Run syntax tests.**

Run: `cargo test -p alpha-syntax --test element_types && cargo test -p alpha-syntax`

Expected: all tests pass.

- [x] **Step 6: Commit.**

```bash
git add alpha-syntax/src/token_kind.rs alpha-syntax/src/syntax_kind.rs \
  alpha-syntax/src/parser/items.rs alpha-syntax/src/ast.rs \
  alpha-syntax/tests/element_types.rs
git commit -m "feat: parse Alpha element types"
```

---

### Task 2: Resolve Types And Enforce Linear Qubits

**Files:**
- Create: `alpha-model/src/ty.rs`
- Modify: `alpha-model/src/lib.rs`
- Modify: `alpha-model/src/resolve.rs`
- Modify: `alpha-model/src/analyze.rs`
- Modify: `alpha-model/src/diagnostic.rs`
- Create: `alpha-model/tests/element_types.rs`
- Modify: `alpha-transform/src/ir.rs`
- Modify: `alpha-transform/src/lower.rs`
- Modify: `alpha-transform/src/print.rs`
- Modify: `alpha-transform/tests/print_fixtures.rs`

**Interfaces:**
- Produces: `alpha_model::ElementType::{Unspecified, Bool, Int, Real, Qubit}`.
- Produces: `Resolver::variable_type(&self, name: &str) -> Option<ElementType>`.
- Changes: `alpha_transform::ir::Variable` gains `element_type: ElementType`.
- Preserves: existing untyped declarations resolve to `ElementType::Unspecified`.

- [x] **Step 1: Add failing model tests.**

```rust
#[test]
fn explicitly_linear_qubits_are_valid() {
    let source = r#"affine q [] -> {:}
    inputs linear Q : {[i] : 0 <= i < 2} of qubit;
    outputs linear R : {[i] : 0 <= i < 2} of qubit;
    let R[i] = Q[i];
.
"#;
    assert!(alpha_model::check_source(source).is_empty());
}

#[test]
fn unrestricted_qubits_are_rejected() {
    let source = r#"affine q [] -> {:}
    inputs Q : {[i] : 0 <= i < 2} of qubit;
    outputs Y : {[i] : 0 <= i < 2};
    let Y[i] = 0;
.
"#;
    let diagnostics = alpha_model::check_source(source);
    assert!(diagnostics.iter().any(|(_, d)| matches!(
        d,
        alpha_model::Diagnostic::QubitMustBeLinear { variable, .. } if variable == "Q"
    )));
}
```

Also assert comma-group type inheritance and that existing untyped fixtures remain diagnostic-free.

- [x] **Step 2: Run the focused tests and confirm failure.**

Run: `cargo test -p alpha-model --test element_types`

Expected: compile failure because model element types and the diagnostic are absent.

- [x] **Step 3: Implement model type resolution.**

Create `ty.rs`:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ElementType {
    #[default]
    Unspecified,
    Bool,
    Int,
    Real,
    Qubit,
}
```

Populate `Resolver::variable_types` in the same comma-group scan as multiplicities. A bare name
inherits the terminating declaration's type. Add `QubitMustBeLinear { variable, start, end }` and
run the check after declarations are resolved and before multiplicity expression analysis.

- [x] **Step 4: Carry types through transform IR and printers.**

Set each lowered variable's `element_type` from `Resolver::variable_type`. Make `show`/`ashow`
render ` of bool|int|real|qubit` only when the type is not `Unspecified`; include the type in the
debug printer.

- [x] **Step 5: Run model and transform tests.**

Run: `cargo test -p alpha-model --test element_types && cargo test -p alpha-model && cargo test -p alpha-transform`

Expected: all tests pass, including existing untyped fixtures.

- [x] **Step 6: Commit.**

```bash
git add alpha-model/src/ty.rs alpha-model/src/lib.rs alpha-model/src/resolve.rs \
  alpha-model/src/analyze.rs alpha-model/src/diagnostic.rs alpha-model/tests/element_types.rs \
  alpha-transform/src/ir.rs alpha-transform/src/lower.rs alpha-transform/src/print.rs \
  alpha-transform/tests/print_fixtures.rs
git commit -m "feat: type Alpha qubit variables"
```

---

### Task 3: Add The Quantum Operation Registry And Typed Call Checking

**Files:**
- Create: `alpha-model/src/operation.rs`
- Modify: `alpha-model/src/lib.rs`
- Modify: `alpha-model/src/analyze.rs`
- Modify: `alpha-model/src/multiplicity.rs`
- Modify: `alpha-model/src/diagnostic.rs`
- Create: `alpha-model/tests/quantum_calls.rs`

**Interfaces:**
- Produces: `RegisteredOperation::{QAlloc, H, Cx, Measure, Discard}`.
- Produces: `registered_operation(name: &str) -> Option<OperationSignature>`.
- Produces: typed `Port`, `Continuity`, and `OperationSignature` shared by analysis and lowering.
- Changes: existing `PortSignature` data is represented with typed ports; legacy external and system ports use `ElementType::Unspecified`.

- [ ] **Step 1: Add failing registry and source-analysis tests.**

```rust
#[test]
fn registry_records_quantum_continuity() {
    let cx = alpha_model::registered_operation("cx").unwrap();
    assert_eq!(cx.operation, alpha_model::RegisteredOperation::Cx);
    assert_eq!(cx.continuity, vec![
        alpha_model::Continuity { input: 0, output: 0 },
        alpha_model::Continuity { input: 1, output: 1 },
    ]);
}

#[test]
fn typed_quantum_calls_are_valid_without_external_declarations() {
    let source = r#"affine gates [] -> {:}
    inputs linear Q0, R0 : {[i] : 0 <= i < 2} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < 2} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;
    assert!(alpha_model::check_source(source).is_empty());
}
```

Add tests for `qalloc`, `h`, consuming `measure`, `discard`, wrong arity, qubit/bool mismatch,
linear aliasing in `cx(Q[i], Q[i])`, registered-name redeclaration, and using a registered quantum
operation as a scalar expression. Add `rejects_measurement_controlled_gate_expression` using
`Q1[i] = if M[i] then h(Q[i]) else Q[i];`; it must report that registered quantum operations are
call equations, not assign accidental measurement-control semantics.

- [ ] **Step 2: Run the focused tests and confirm failure.**

Run: `cargo test -p alpha-model --test quantum_calls`

Expected: failures because registered operations do not resolve.

- [ ] **Step 3: Implement the registry types and fixed signatures.**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegisteredOperation { QAlloc, H, Cx, Measure, Discard }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub element_type: ElementType,
    pub multiplicity: Multiplicity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Continuity { pub input: usize, pub output: usize }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSignature {
    pub operation: RegisteredOperation,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub continuity: Vec<Continuity>,
}
```

Return the five signatures exactly as specified in the design. Keep signature lookup independent
of HUGR crates.

- [ ] **Step 4: Integrate typed call validation.**

Resolve registered names before falling back to source-declared systems/externals. Reject a
source declaration that reserves `qalloc`, `h`, `cx`, `measure`, or `discard`. Add explicit
diagnostics for call arity, port element-type mismatch, invalid operation context,
`CallDomainMismatch`, and aliased linear operands. For a registered call without an explicit
`over`, require every direct port access to expose the same consumer domain; reject disagreement
instead of intersecting domains silently. Reuse `collect_use_equation_expr` for exact affine
resource relations; do not add a parallel approximate traversal.

- [ ] **Step 5: Run model regressions.**

Run: `cargo test -p alpha-model --test quantum_calls && cargo test -p alpha-model`

Expected: all tests pass, including legacy external multiplicity tests.

- [ ] **Step 6: Commit.**

```bash
git add alpha-model/src/operation.rs alpha-model/src/lib.rs alpha-model/src/analyze.rs \
  alpha-model/src/multiplicity.rs alpha-model/src/diagnostic.rs \
  alpha-model/tests/quantum_calls.rs
git commit -m "feat: type registered quantum operations"
```

---

### Task 4: Lower Quantum Calls Into Explicit IR Statements

**Files:**
- Modify: `alpha-transform/src/ir.rs`
- Modify: `alpha-transform/src/lower.rs`
- Modify: `alpha-transform/src/normalize.rs`
- Modify: `alpha-transform/src/normalize_reduction.rs`
- Modify: `alpha-transform/src/print.rs`
- Create: `alpha-transform/tests/quantum_calls.rs`
- Modify: `alpha-codegen/src/stmt.rs`
- Modify: `alpha-codegen/src/describe.rs`
- Create: `alpha-codegen/tests/quantum_statements.rs`

**Interfaces:**
- Produces: `ir::Access { variable: String, function: MultiAff }`.
- Produces: `ir::OperationCall { operation, index_names, domain, inputs, outputs }`.
- Changes: `ir::Equation` gains `OperationCall`; unresolved subsystem calls remain `Use`.
- Produces: `StatementKind::OperationCall(&ir::OperationCall)` with deterministic names such as `Q1__call0` and `discard__call0`.

- [ ] **Step 1: Add failing lowering and statement tests.**

Use this fixture:

```rust
const GATES: &str = r#"affine gates [N] -> {:N>0}
    inputs linear Q0, R0 : {[i] : 0 <= i < N} of qubit;
    outputs linear Q1, R1 : {[i] : 0 <= i < N} of qubit;
    let with [i] : (Q1[i], R1[i]) = cx(Q0[i], R0[i]);
.
"#;
```

Assert lowering yields one `Equation::OperationCall`, its domain is `[N] -> { [i] : 0 <= i < N }`,
its four accesses are identity maps over `i`, and statement extraction yields `Q1__call0`.
Add a zero-output discard fixture and assert `discard__call0`. Add two disjoint calls targeting the
same first output and assert collision-free source-order suffixes.

- [ ] **Step 2: Run focused tests and confirm failure.**

Run: `cargo test -p alpha-transform --test quantum_calls && cargo test -p alpha-codegen --test quantum_statements`

Expected: compile failure because the operation-call IR does not exist.

- [ ] **Step 3: Add explicit operation-call IR.**

```rust
#[derive(Clone)]
pub struct Access {
    pub variable: String,
    pub function: MultiAff,
}

#[derive(Clone)]
pub struct OperationCall {
    pub operation: alpha_model::RegisteredOperation,
    pub index_names: Vec<String>,
    pub domain: Set,
    pub inputs: Vec<Access>,
    pub outputs: Vec<Access>,
}
```

Lower registered calls to this variant. Derive the call domain from the explicit `over` domain
them. Preserve ordinary `UseEquation` lowering for subsystem calls.
when present; otherwise use the equal consumer domain already proved by Task 3. Treat a mismatch at
this point as an internal lowering invariant violation. Preserve ordinary `UseEquation` lowering
for subsystem calls.

- [ ] **Step 4: Preserve operation calls through normalization and printing.**

Normalization copies access maps and domains unchanged. `show`/`ashow` reconstruct the existing
`with [indices] : (outputs) = operation(inputs);` syntax. The debug printer includes operation,
domain, and access maps.

- [ ] **Step 5: Add operation statements and deterministic naming.**

Extend statement extraction to include every operation call. Use the first output variable as the
base, or the operation name for zero-output calls, append `__call<n>`, and increment `n` until the
name avoids every ordinary, reduction, and earlier call statement. Update `describe` so users see
the generated names and exact domains before writing schedules.

- [ ] **Step 6: Run transform and statement tests.**

Run: `cargo test -p alpha-transform && cargo test -p alpha-codegen --test quantum_statements`

Expected: all tests pass.

- [ ] **Step 7: Commit.**

```bash
git add alpha-transform/src/ir.rs alpha-transform/src/lower.rs \
  alpha-transform/src/normalize.rs alpha-transform/src/normalize_reduction.rs \
  alpha-transform/src/print.rs alpha-transform/tests/quantum_calls.rs \
  alpha-codegen/src/stmt.rs alpha-codegen/src/describe.rs \
  alpha-codegen/tests/quantum_statements.rs
git commit -m "feat: lower quantum call statements"
```

---

### Task 5: Build Backend-Neutral Resource Flow

**Files:**
- Create: `alpha-transform/src/resource_flow.rs`
- Modify: `alpha-transform/src/lib.rs`
- Create: `alpha-transform/tests/resource_flow.rs`

**Interfaces:**
- Consumes: normalized `ir::System` with `OperationCall` equations.
- Produces: `resource_flow::analyze(&ir::System) -> Result<ResourceFlow, ResourceFlowError>`.
- Produces: exact `ContinuityEdge`, `ResourceRoot`, and `ResourceSink` relations over logical variable domains.

- [ ] **Step 1: Add failing trajectory tests.**

Create a fixture with allocation, two time steps of `h`, and consuming measurement:

```alpha
affine chain [T,N] -> {:T=3 and N>0}
outputs M : {[i] : 0 <= i < N} of bool;
locals linear Q : {[t,i] : 0 <= t < T and 0 <= i < N} of qubit;
let
    over {[t,i] : t=0 and 0<=i<N} with [t,i] : (Q[t,i]) = qalloc();
    over {[t,i] : 0<t<T and 0<=i<N} with [t,i] : (Q[t,i]) = h(Q[t-1,i]);
    with [i] : (M[i]) = measure(Q[T-1,i]);
.
```

Assert one allocation-root relation over `i`, one measurement-sink relation, and continuity:

```text
[T,N] -> { Q[t,i] -> Q[1+t,i] : 0 <= t < T-1 and 0 <= i < N }
```

Add a `cx` test proving two independent continuity relations and a direct input-to-output test.

- [ ] **Step 2: Run the focused tests and confirm failure.**

Run: `cargo test -p alpha-transform --test resource_flow`

Expected: compile failure because `resource_flow` is absent.

- [ ] **Step 3: Implement relational flow extraction.**

Define:

```rust
pub struct ContinuityEdge {
    pub statement: String,
    pub input_variable: String,
    pub output_variable: String,
    pub relation: Map,
}

pub enum ResourceRootKind { SystemInput, OperationOutput(RegisteredOperation) }
pub enum ResourceSinkKind { SystemOutput, OperationInput(RegisteredOperation) }

pub struct ResourceFlow {
    pub edges: Vec<ContinuityEdge>,
    pub roots: Vec<ResourceRoot>,
    pub sinks: Vec<ResourceSink>,
}
```

For continuity `p -> q`, compute `input.function.into_map().reverse()?.apply_range(output.function.into_map()?)` and restrict it to the call domain. Classify unpaired operation outputs as roots and unpaired inputs as sinks. Add system-boundary roots/sinks for linear qubit variables.

- [ ] **Step 4: Validate flow invariants exactly.**

Use ISL injectivity, range intersections, and domain/range equality to reject branching,
convergence, incomplete endpoint coverage, or type-changing continuity. Return errors containing
the statement and witness relation. Do not enumerate specialized points.

- [ ] **Step 5: Run resource-flow and linearity regressions.**

Run: `cargo test -p alpha-transform --test resource_flow && cargo test -p alpha-transform && cargo test -p alpha-model --test multiplicity`

Expected: all tests pass.

- [ ] **Step 6: Commit.**

```bash
git add alpha-transform/src/resource_flow.rs alpha-transform/src/lib.rs \
  alpha-transform/tests/resource_flow.rs
git commit -m "feat: derive logical quantum resource flow"
```

---

### Task 6: Introduce Typed Scheduled IR And Migrate Scheduled C

**Files:**
- Create: `alpha-codegen/src/scheduled_ir.rs`
- Modify: `alpha-codegen/src/lib.rs`
- Modify: `alpha-codegen/src/scheduledc.rs`
- Modify: `alpha-codegen/src/error.rs`
- Create: `alpha-codegen/tests/scheduled_ir.rs`
- Update: `alpha-codegen/tests/snapshots/*.snap` only if formatting intentionally changes

**Interfaces:**
- Produces: public `scheduled_ir::build(system, schedule_text) -> Result<ScheduledProgram<'_>>`.
- Produces: typed `ScheduledNode`, `IndexExpr`, `Predicate`, and `StatementId`.
- Changes: ScheduledC renders `ScheduledProgram` rather than walking `isl::AstNode` directly.

- [ ] **Step 1: Add failing scheduled-IR snapshots.**

Cover identity, reverse, skewed two-dimensional, affine guard, reduction, and operation-call
schedules. Assert structure directly, for example:

```rust
let program = alpha_codegen::scheduled_ir::build(&system, "[N] -> { Y[i] -> [N-1-i]; }").unwrap();
insta::assert_debug_snapshot!(program.root);
assert!(matches!(program.root, ScheduledNode::Loop { .. }));
```

Include an operation invocation assertion using `StatementId` rather than a name string.

- [ ] **Step 2: Run the focused tests and confirm failure.**

Run: `cargo test -p alpha-codegen --test scheduled_ir`

Expected: compile failure because `scheduled_ir` is absent.

- [ ] **Step 3: Define the typed IR and ISL expression conversion.**

Use owned enums:

```rust
pub enum IndexExpr {
    Constant(i64),
    Variable(String),
    Add(Box<IndexExpr>, Box<IndexExpr>),
    Sub(Box<IndexExpr>, Box<IndexExpr>),
    Mul(Box<IndexExpr>, Box<IndexExpr>),
    FloorDiv(Box<IndexExpr>, Box<IndexExpr>),
    CeilDiv(Box<IndexExpr>, Box<IndexExpr>),
    Mod(Box<IndexExpr>, Box<IndexExpr>),
    Min(Vec<IndexExpr>),
    Max(Vec<IndexExpr>),
}

pub enum Predicate {
    Compare { op: CompareOp, lhs: IndexExpr, rhs: IndexExpr },
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
    Constant(bool),
}
```

Map every `isl::AstExprKind` and supported `isl::AstOpType` explicitly. Reject call/select/address
operators in loop bounds and predicates except the user-call expression handled by `AstNode::User`.

- [ ] **Step 4: Translate the ISL AST once.**

Move parameter-context construction and `AstBuild::generate` out of `scheduledc.rs`. Convert
`For`, `If`, `Block`, and `User` into `ScheduledNode`; resolve user tuple names to stable
`StatementId`s during translation. Return the statements and root together as `ScheduledProgram`.

- [ ] **Step 5: Migrate ScheduledC to the typed IR.**

Replace its ISL walker with renderers for `IndexExpr`, `Predicate`, and `ScheduledNode`. Keep
statement body generation, storage layout, and expression rendering unchanged.

- [ ] **Step 6: Run all scheduled-C checks.**

Run:

```bash
cargo test -p alpha-codegen --test scheduled_ir
cargo test -p alpha-codegen --test scheduledc_snapshots
cargo test -p alpha-codegen --test scheduledc_e2e
cargo test -p alpha-codegen
```

Expected: existing snapshots and executable behavior pass unchanged. Review any intentional
snapshot diff before accepting it.

- [ ] **Step 7: Commit.**

```bash
git add alpha-codegen/src/scheduled_ir.rs alpha-codegen/src/lib.rs \
  alpha-codegen/src/scheduledc.rs alpha-codegen/src/error.rs \
  alpha-codegen/tests/scheduled_ir.rs alpha-codegen/tests/snapshots
git commit -m "refactor: add backend-neutral scheduled IR"
```

---

### Task 7: Specialize Alpha Parameters For HUGR

**Files:**
- Modify: `isl/src/set.rs`
- Modify: `isl/src/map.rs`
- Modify: `isl/src/union_map.rs`
- Modify: `isl/tests/smoke.rs`
- Create: `alpha-codegen/src/specialize.rs`
- Modify: `alpha-codegen/src/lib.rs`
- Modify: `alpha-codegen/src/error.rs`
- Create: `alpha-codegen/tests/specialization.rs`

**Interfaces:**
- Produces: `type ParameterBindings = BTreeMap<String, i64>`.
- Produces: `specialize::apply(program: &ScheduledProgram<'_>, bindings: &ParameterBindings) -> Result<SpecializedProgram<'_>>`.
- Produces: exact ISL restriction helpers needed to intersect sets/maps/union maps with one parameter point.
- Produces: `CodegenError::Specialization(String)`.

- [ ] **Step 1: Add failing ISL wrapper and specialization tests.**

In `specializes_parameter_point`, assert `[N] -> { [i] : 0 <= i < N }` specialized with `N=4`
equals `{ [i] : 0 <= i <= 3 }` after parameter projection. At codegen level, assert missing `N`,
`N=0` outside `N>0`, and unknown `M` each yield distinct `CodegenError::Specialization` messages.

- [ ] **Step 2: Run focused tests and confirm failure.**

Run: `cargo test -p isl --test smoke specializes_parameter_point && cargo test -p alpha-codegen --test specialization`

Expected: failure because parameter restriction APIs and specialization do not exist.

- [ ] **Step 3: Add safe ISL parameter-restriction wrappers.**

Wrap the relevant `isl_*_intersect_params` and parameter projection calls for `Set`, `Map`, and
`UnionMap`, following existing ownership/error patterns. Build a singleton parameter set from the
system's ordered parameter names and supplied integer values; do not perform textual substitution
inside arbitrary ISL expressions.

- [ ] **Step 4: Implement specialization validation.**

`specialize::apply` must:

1. reject missing bindings for referenced parameters;
2. reject unknown binding names;
3. prove the singleton point is contained in `system.parameter_domain`;
4. intersect statement domains, resource relations, and the validated schedule with that point;
5. project fixed parameter dimensions where required by HUGR shape computation;
6. retain the concrete values for index-expression lowering.

- [ ] **Step 5: Run focused and codegen tests.**

Run: `cargo test -p isl && cargo test -p alpha-codegen --test specialization && cargo test -p alpha-codegen`

Expected: all tests pass.

- [ ] **Step 6: Commit.**

```bash
git add isl/src/set.rs isl/src/map.rs isl/src/union_map.rs isl/tests/smoke.rs \
  alpha-codegen/src/specialize.rs alpha-codegen/src/lib.rs alpha-codegen/src/error.rs \
  alpha-codegen/tests/specialization.rs
git commit -m "feat: specialize scheduled Alpha programs"
```

---

### Task 8: Infer Rectangular Quantum Realizations

**Files:**
- Create: `alpha-codegen/src/realize.rs`
- Modify: `alpha-codegen/src/lib.rs`
- Modify: `alpha-codegen/src/error.rs`
- Create: `alpha-codegen/tests/realization.rs`

**Interfaces:**
- Consumes: `SpecializedProgram` and `alpha_transform::resource_flow::ResourceFlow`.
- Produces: public `realize::infer(&SpecializedProgram, &ResourceFlow) -> Result<Realization>`.
- Produces: `ResourceGroup`, `LogicalLaneMap`, concrete shape/size, root occupancy, and sink occupancy.
- Produces: `CodegenError::Realization(String)`.

- [ ] **Step 1: Add failing compact-realization tests.**

For the `Q[t,i]` chain with `T=3, N=4`, assert:

```rust
let realization = infer(&specialized, &flow).unwrap();
assert_eq!(realization.groups.len(), 1);
assert_eq!(realization.groups[0].size, 4);
assert_eq!(realization.logical_lane_map("Q").unwrap().to_string(),
           "{ [t, i] -> [i] : 0 <= t <= 2 and 0 <= i <= 3 }");
```

Add tests for input-root/output-sink, allocation-root/measurement-sink, `cx` preserving two lanes,
operand alias rejection, triangular-domain rejection, and mixed non-rectangular sink rejection.

- [ ] **Step 2: Run the focused test and confirm failure.**

Run: `cargo test -p alpha-codegen --test realization`

Expected: compile failure because realization types are absent.

- [ ] **Step 3: Define realization data.**

```rust
pub struct ResourceGroup {
    pub id: ResourceGroupId,
    pub element_type: ElementType,
    pub shape: Vec<u64>,
    pub size: u64,
    pub entry: EntryOccupancy,
    pub exit: ExitOccupancy,
}

pub struct LogicalLaneMap {
    pub variable: String,
    pub group: ResourceGroupId,
    pub relation: Map,
}

pub struct Realization {
    pub groups: Vec<ResourceGroup>,
    pub logical_to_lane: Vec<LogicalLaneMap>,
}

impl Realization {
    pub fn logical_lane_map(&self, variable: &str) -> Option<&Map> {
        self.logical_to_lane
            .iter()
            .find(|entry| entry.variable == variable)
            .map(|entry| &entry.relation)
    }
}
```

- [ ] **Step 4: Implement trajectory propagation.**

Assign each system-input domain and each unpaired operation-output root a distinct root lane space.
Propagate its lane map through each continuity relation. At joins, require exact equality of the
maps already inferred; reject branching, convergence, cycles without a boundary root, and
unreachable logical points with a witness relation.

- [ ] **Step 5: Implement rectangular grouping and boundary checks.**

Accept a group only when its specialized root domain is a zero-based rectangular product and the
logical-to-lane map is an affine projection/permutation onto that product. Compute row-major size
with checked `u64` multiplication. Partition by element type, root kind, sink kind, and compatible
shape; reject a remainder that is not itself rectangular. Never create a dense group over a full
logical version domain as fallback.

- [ ] **Step 6: Prove operation occupancy preconditions.**

For each scheduled call, prove each borrow follows its trajectory root and predecessor, each return
is paired with the same invocation's borrow or an empty allocation-root lane, gate operands map to
distinct lanes, output groups are full, and consumed groups are empty. Store the per-statement
`ResolvedAccess { group, lane: MultiAff }` needed by HUGR emission.

- [ ] **Step 7: Run realization and schedule regressions.**

Run: `cargo test -p alpha-codegen --test realization && cargo test -p alpha-codegen`

Expected: all tests pass.

- [ ] **Step 8: Commit.**

```bash
git add alpha-codegen/src/realize.rs alpha-codegen/src/lib.rs \
  alpha-codegen/src/error.rs alpha-codegen/tests/realization.rs
git commit -m "feat: infer compact quantum realizations"
```

---

### Task 9: Add Tested HUGR Emission Primitives

**Files:**
- Modify: `Cargo.toml`
- Modify: `alpha-codegen/Cargo.toml`
- Modify: `Cargo.lock`
- Create: `alpha-codegen/src/hugr.rs`
- Modify: `alpha-codegen/src/lib.rs`
- Modify: `alpha-codegen/src/error.rs`
- Create: `alpha-codegen/tests/hugr_primitives.rs`

**Interfaces:**
- Adds: `hugr = "0.29.3"` and `tket = "0.21.2"` workspace dependencies.
- Produces: private helpers for borrow-array conversion/access, `TketOp` gates, typed index arithmetic, and counted `TailLoop` construction.
- Produces: `CodegenError::Hugr(String)`.

- [ ] **Step 1: Add dependencies and a failing primitive test.**

Add the two exact versions to `[workspace.dependencies]` and reference them with `.workspace = true`
in `alpha-codegen`. Do not add `tket-qsystem`: the required operations are `TketOp::{QAlloc, H,
CX, MeasureFree, QFree}`.

Write a test that creates `borrow_array<4, qubit>`, borrows index 1, applies `H`, returns the output,
finishes the DFG, and calls HUGR validation.

- [ ] **Step 2: Run the primitive test and confirm failure.**

Run: `cargo test -p alpha-codegen --test hugr_primitives`

Expected: compile failure because the HUGR module/helpers do not exist.

- [ ] **Step 3: Implement borrow-array and gate helpers.**

Use `BArrayOpBuilder::{add_borrow_array_borrow, add_borrow_array_return,
add_new_all_borrowed, add_discard_all_borrowed}`. Add dataflow ops using:

```rust
TketOp::QAlloc
TketOp::H
TketOp::CX
TketOp::MeasureFree
TketOp::QFree
```

For `MeasureFree`, use the TKET measurement extension's read operation to convert the measurement
value to the Alpha `bool` result expected by the registry. Keep this conversion inside the HUGR
backend; the Alpha operation signature remains `qubit -> bool`.

- [ ] **Step 4: Implement typed index-expression lowering.**

Map `IndexExpr` to HUGR integer constants/arithmetic and `Predicate` to comparison/logic ops. Use
checked conversions for negative values and division semantics; add a test for every variant
produced by the scheduled-IR fixture suite.

- [ ] **Step 5: Implement a counted TailLoop skeleton.**

Build a `TailLoop` carrying an iterator plus one borrow array in its `rest` row. Emit condition,
continue, increment, and break paths from a `ScheduledNode::Loop`; assert the resulting HUGR
contains a `TailLoop` and validates.

- [ ] **Step 6: Run primitive tests and strict linting for the crate.**

Run: `cargo test -p alpha-codegen --test hugr_primitives && cargo clippy -p alpha-codegen --all-targets -- -D warnings`

Expected: all tests and clippy pass.

- [ ] **Step 7: Commit.**

```bash
git add Cargo.toml Cargo.lock alpha-codegen/Cargo.toml alpha-codegen/src/hugr.rs \
  alpha-codegen/src/lib.rs alpha-codegen/src/error.rs \
  alpha-codegen/tests/hugr_primitives.rs
git commit -m "feat: add HUGR emission primitives"
```

---

### Task 10: Emit Complete Scheduled Quantum HUGRs

**Files:**
- Modify: `alpha-codegen/src/hugr.rs`
- Modify: `alpha-codegen/src/lib.rs`
- Create: `alpha-codegen/tests/src/quantum_chain.alpha`
- Create: `alpha-codegen/tests/src/quantum_cx.alpha`
- Create: `alpha-codegen/tests/hugr_scheduled.rs`
- Create: `alpha-codegen/tests/snapshots/hugr_scheduled__*.snap`

**Interfaces:**
- Produces: `generate_hugr(system, schedule_text, bindings) -> Result<hugr::Hugr>`.
- Produces: `generate_hugr_system(system, schedule_text, bindings) -> Result<String>` using HUGR envelope serialization.
- Consumes: statement table, resource flow, scheduled IR, specialization, and realization.

- [ ] **Step 1: Add failing end-to-end tests.**

Test an allocated `Q[t,i]` chain ending in measurement and an input/output `cx` program. For each:

```rust
let hugr = alpha_codegen::generate_hugr(&system, schedule, &bindings).unwrap();
hugr.validate().unwrap();
assert!(hugr.nodes().any(|node| hugr.get_optype(node).is_tail_loop()));
```

Inspect operations to assert the expected counts of `QAlloc`, `H`, `CX`, `MeasureFree`, borrow, and
return nodes. Assert the public function signature uses concrete arrays of the specialized sizes.
Add a second legal schedule and assert both HUGRs validate but have different loop structure.

- [ ] **Step 2: Run end-to-end tests and confirm failure.**

Run: `cargo test -p alpha-codegen --test hugr_scheduled`

Expected: failure because whole-program emission is absent.

- [ ] **Step 3: Build HUGR boundary state.**

Create a DFG with concrete input/output array types. Convert input resource groups to borrow arrays,
create allocation-root groups with `new_all_borrowed`, and create empty borrow arrays for
write-once classical outputs. Store group wires in a deterministic `ResourceGroupId` map.

- [ ] **Step 4: Walk scheduled IR with explicit live state.**

For `Sequence`, thread the state map through children. For `Loop`, compute values live across the
back edge and place them in the `TailLoop.rest` row. For `If`, pass identical typed rows into both
cases and merge their outputs. For `Invoke`, evaluate lane indices, borrow inputs, emit the
operation, and return continuity outputs. A consuming input remains borrowed; a root output is
returned into its initially empty group.

- [ ] **Step 5: Finish boundaries and validate.**

Convert proved-full quantum and classical output groups to ordinary arrays. Discard proved-empty
consumed groups. Finish the HUGR, run `validate`, and map validation failures to
`CodegenError::Hugr`. Serialize with `hugr::envelope::EnvelopeConfig` in
`generate_hugr_system`.

- [ ] **Step 6: Pin unsupported behavior.**

Add tests asserting dedicated errors for:

- missing specialization bindings;
- triangular qubit domains;
- unproved compact realization;
- an operation absent from the registry;
- an unsupported scheduled index expression.

Name the first three tests `rejects_missing_specialization`, `rejects_triangular_domain`, and
`rejects_unproved_compact_realization`. Measurement-controlled gate expressions are rejected in
Task 3 before codegen and remain covered by `rejects_measurement_controlled_gate_expression`.

- [ ] **Step 7: Run HUGR and full codegen suites.**

Run:

```bash
cargo test -p alpha-codegen --test hugr_scheduled
cargo test -p alpha-codegen --test hugr_primitives
cargo test -p alpha-codegen
```

Expected: all tests pass. Read every new HUGR snapshot and verify that qubit wires are consumed
exactly once; do not approve snapshots solely because `UPDATE_INSTA=1` generated them.

- [ ] **Step 8: Commit.**

```bash
git add alpha-codegen/src/hugr.rs alpha-codegen/src/lib.rs \
  alpha-codegen/tests/src/quantum_chain.alpha alpha-codegen/tests/src/quantum_cx.alpha \
  alpha-codegen/tests/hugr_scheduled.rs alpha-codegen/tests/snapshots
git commit -m "feat: emit scheduled quantum HUGRs"
```

---

### Task 11: Expose HUGR Generation In CLI And Python

**Files:**
- Modify: `alphac/src/main.rs`
- Modify: `alphac/Cargo.toml`
- Create: `alphac/tests/cli_hugr.rs`
- Modify: `alphalang/src/lib.rs`
- Modify: `alphalang/python/alphalang/__init__.py`
- Modify: `alphalang/tests/test_alpha.py`
- Modify: `alphalang/README.md`
- Modify: `alpha-codegen/README.md`
- Create: `alphalang/notebooks/quantum_hugr.ipynb`
- Modify: `alphalang/notebooks/README.md`

**Interfaces:**
- CLI: `alphac --emit hugr --schedule schedule.isl --param N=4 --param T=3 input.alpha`.
- Python: `alphalang.generate_hugr(system, parameters) -> str`, accepting `NormalizedSystem` or `ScheduledSystem` and using the latter's stored schedule.
- Python metadata: `Variable.element_type` and exported `ElementType` enum.

- [ ] **Step 1: Add failing Python API tests.**

```python
def test_qubit_metadata_and_hugr_generation():
    norm = alphalang.normalize(alphalang.parse(QUANTUM_CHAIN))
    scheduled = norm.schedule(QUANTUM_CHAIN_SCHEDULE)
    assert scheduled.locals[0].element_type is alphalang.ElementType.QUBIT
    envelope = alphalang.generate_hugr(scheduled, {"T": 3, "N": 4})
    assert isinstance(envelope, str)
    assert "TailLoop" in envelope


def test_hugr_generation_requires_parameters():
    norm = alphalang.normalize(alphalang.parse(QUANTUM_CHAIN))
    with pytest.raises(ValueError, match="missing.*N"):
        alphalang.generate_hugr(norm, {})
```

Also assert bare `System` raises `TypeError`, invalid schedules remain `ScheduleError`, and
realization/HUGR failures become `ValueError` with the Rust diagnostic intact.

- [ ] **Step 2: Add failing CLI integration tests.**

Add `alphac/tests/cli_hugr.rs` if the crate has no CLI test target yet. Invoke the binary with the
quantum fixture, schedule file, and repeated `--param`; assert success and parse/validate the
serialized envelope. Assert omitted `N` exits unsuccessfully and names the missing binding.

- [ ] **Step 3: Run binding and CLI tests to confirm failure.**

Run: `uv run pytest alphalang/tests/test_alpha.py -q && cargo test -p alphac --test cli_hugr`

Expected: failures because the APIs and flags are absent.

- [ ] **Step 4: Implement Python bindings.**

Add frozen `ElementType` values mirroring Rust. Extend `Variable` snapshots with `element_type`.
Implement `generate_hugr` with the same stage checks as `generate`: a `ScheduledSystem` uses its
stored schedule, and a `NormalizedSystem` uses the identity schedule. Convert `dict[str, int]` to
`BTreeMap<String, i64>` and return the serialized envelope.

- [ ] **Step 5: Implement CLI flags without changing the default C path.**

Extend `Args` with:

```rust
enum Emit { C, Hugr }
struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
    emit: Emit,
    schedule: Option<PathBuf>,
    parameters: BTreeMap<String, i64>,
}
```

Default `--emit` to `c`. For `hugr`, read schedule text when supplied, otherwise use the identity
schedule, and call `generate_hugr_system`. Reject duplicate/malformed `--param NAME=VALUE` entries
with usage errors.

- [ ] **Step 6: Add an executed notebook and documentation.**

The notebook must show:

- explicit `linear ... of qubit` declarations;
- allocation, `h`, `cx`, consuming measurement, and discard;
- variable type/multiplicity metadata;
- schedule inspection and one alternate legal schedule;
- concrete parameter bindings;
- generated HUGR text;
- rejection of a non-linear qubit and an unsupported irregular realization;
- a note that measurement-dependent control and parametric HUGR are deferred.

Execute it in place and preserve deterministic outputs and cell IDs/language metadata.

- [ ] **Step 7: Run public-surface validation.**

Run:

```bash
uv run maturin develop --manifest-path alphalang/Cargo.toml
uv run pytest alphalang/tests -q
uv run pytest --nbval alphalang/notebooks/prefix_sum.ipynb -q
uv run pytest --nbval alphalang/notebooks/linear_types.ipynb -q
uv run pytest --nbval alphalang/notebooks/quantum_hugr.ipynb -q
cargo test -p alphac --test cli_hugr
```

Expected: all tests pass.

- [ ] **Step 8: Commit.**

```bash
git add alphac/src/main.rs alphac/Cargo.toml alphac/tests/cli_hugr.rs \
  alphalang/src/lib.rs alphalang/python/alphalang/__init__.py \
  alphalang/tests/test_alpha.py alphalang/README.md alpha-codegen/README.md \
  alphalang/notebooks/quantum_hugr.ipynb alphalang/notebooks/README.md
git commit -m "feat: expose scheduled HUGR generation"
```

---

### Task 12: Final Integration And Deferred-Feature Guards

**Files:**
- Modify: `docs/superpowers/plans/2026-08-29-alpha-scheduled-hugr.md`
- Modify only if validation exposes task-related defects: files changed in Tasks 1-11

**Interfaces:**
- Consumes: the complete scheduled HUGR pipeline.
- Produces: a fully validated branch with every task checkbox updated and deferred behavior pinned by tests.

- [ ] **Step 1: Run formatting and strict linting.**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both commands pass without warnings.

- [ ] **Step 2: Run the full Rust workspace suite.**

Run: `cargo test --workspace --no-fail-fast`

Expected: all unit, integration, and doc tests pass.

- [ ] **Step 3: Run the complete Python and notebook suite.**

```bash
uv run pytest alphalang/tests -q
uv run pytest --nbval alphalang/notebooks/prefix_sum.ipynb -q
uv run pytest --nbval alphalang/notebooks/linear_types.ipynb -q
uv run pytest --nbval alphalang/notebooks/quantum_hugr.ipynb -q
```

Expected: all tests pass.

- [ ] **Step 4: Verify deferred-feature diagnostics remain explicit.**

Run the focused tests that assert rejection of irregular realization, missing specialization,
measurement-dependent control, and unknown HUGR operations:

```bash
cargo test -p alpha-codegen --test hugr_scheduled rejects_missing_specialization
cargo test -p alpha-codegen --test realization rejects_triangular_domain
cargo test -p alpha-codegen --test realization rejects_unproved_compact_realization
cargo test -p alpha-model --test quantum_calls rejects_measurement_controlled_gate_expression
```

Expected: all diagnostic assertions pass.

- [ ] **Step 5: Check repository scope and plan completion.**

```bash
git diff --check
git status --short
git diff --stat a6ea94a..HEAD
```

Confirm no unrelated files are staged or modified. Mark completed task steps in this plan as work
lands; the final plan-only checkbox update belongs in the last implementation commit.

- [ ] **Step 6: Commit any final task-related corrections and plan status.**

```bash
git add docs/superpowers/plans/2026-08-29-alpha-scheduled-hugr.md
git commit -m "test: complete scheduled HUGR integration"
```
