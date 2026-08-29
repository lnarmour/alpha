# Scheduled HUGR Code Generation for Alpha

## Status

Approved design. Target branch: `louis/hugr`.

## Goal

Add first-class quantum values and operations to Alpha and compile scheduled Alpha programs to
HUGR. The implementation must preserve Alpha's explicit dataflow semantics while using an ISL
schedule to choose execution order and loop structure.

The design has four central principles:

1. A qubit is an explicitly linear Alpha value.
2. Program equations and operation signatures determine logical resource flow; schedules do not.
3. ISL performs polyhedral scanning, but its AST is translated immediately into an Alpha-owned,
   backend-neutral scheduled IR.
4. HUGR realization may compact many logical versions into fewer live resource lanes, but only
   when the compiler proves the mapping correct.

For example:

```alpha
linear Q : {[t,i] : 0 <= t < T and 0 <= i < N} of qubit;
```

declares `T * N` logical values. It does not require `T * N` physical qubits. If the program forms
`N` chains across `t`, a valid realization may use `N` loop-carried qubit lanes.

## Non-goals

The first implementation does not provide:

- measurement-dependent gate control;
- parametric HUGR generation;
- arbitrary irregular-domain packing;
- reuse of one physical lane between unrelated resource trajectories;
- hardware placement, routing, or device-qubit assignment;
- source syntax for importing arbitrary HUGR extension signatures;
- a replacement for ISL's polyhedral scanner.

These are deferred capabilities, not rejected directions. They are specified explicitly below so
that the initial implementation does not accidentally close them off.

## Why ISL Still Generates the Loop Structure

Reimplementing CLooG or ISL AST generation would require Alpha to own polyhedral scanning,
projection, bounds with floors and ceilings, guards for non-convex domains, and simplification of
multi-statement loop nests. That work is large, subtle, and independent of quantum semantics.

Conversely, lowering directly from ISL nodes to HUGR would couple the backend to ISL conventions
and duplicate traversal already present in `ScheduledC`.

The chosen architecture inserts a structured scheduled IR:

```text
normalized Alpha IR
        |
statement extraction and schedule legality
        |
validated ISL union-map schedule
        |
ISL AST generation
        |
Alpha scheduled IR
   |             |
scheduled C     HUGR realization and emission
```

The ISL AST is not treated as C. It is a structured result containing loops, affine guards,
blocks, and statement invocations. C rendering is only one consumer.

## Surface Language

### Element types

Variables gain an element type independently of multiplicity. The initial semantic representation
is equivalent to:

```rust
pub enum ElementType {
    Unspecified,
    Bool,
    Int,
    Real,
    Qubit,
}

pub struct Variable {
    pub name: String,
    pub domain: Set,
    pub element_type: ElementType,
    pub multiplicity: Multiplicity,
}
```

`Unspecified` preserves compatibility with existing Alpha programs whose numeric type is implicit.
The exact set of classical numeric variants may follow the existing parser's type vocabulary, but
`Qubit` is a distinct first-class type.

A qubit variable must be explicitly declared linear:

```alpha
inputs
    linear Q_in : {[i] : 0 <= i < N} of qubit;
outputs
    linear Q_out : {[i] : 0 <= i < N} of qubit;
```

This is invalid:

```alpha
inputs
    Q : {[i] : 0 <= i < N} of qubit;
```

The type checker reports that a qubit cannot have unrestricted multiplicity. `qubit` does not
silently infer or insert the `linear` modifier. Multiplicity remains a property of the binding;
the qubit type constrains which multiplicities are legal.

### No `inouts` section

The initial language does not add an `inouts` declaration category. A transformed quantum value
is represented by an explicit linear input and output:

```alpha
inputs
    linear Q_in : {[i] : 0 <= i < N} of qubit;
outputs
    linear Q_out : {[i] : 0 <= i < N} of qubit;
locals
    linear Q1 : {[i] : 0 <= i < N} of qubit;
let
    (Q1[i]) = h(Q_in[i]);
    Q_out[i] = Q1[i];
.
```

At the HUGR boundary, `Q_in` and `Q_out` become ordinary input and output ports. An `inout` source
feature may later be added as sugar over two explicit linear ports, but it must not introduce
implicit mutation or schedule-dependent meaning.

### Call equations

Alpha's existing multi-output `UseEquation` surface form is generalized semantically into a typed
call equation. A callee may resolve to either another Alpha system or a registered operation:

```alpha
(Q1[i]) = h(Q0[i]);
(Q2[i], R2[i]) = cx(Q1[i], R1[i]);
(M[i]) = measure(Q2[i]);
() = discard(R2[i]);
```

Calls with linear outputs require each output to be an exact affine access into a declared target
variable. Calls with linear inputs require exact affine input accesses. These restrictions give the
compiler exact definition and consumption relations. Existing unrestricted subsystem calls retain
their current behavior unless stronger typing requires otherwise.

A resolved call records its statement domain, input access relations, output definition relations,
operation identity, and typed signature. Calls are schedulable statements; gates are not embedded
inside scalar expressions.

## Operation Signatures

The operation registry is backend-independent:

```rust
pub struct OperationSignature {
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub continuity: Vec<Continuity>,
    pub implementation: OperationImplementation,
}

pub struct Port {
    pub element_type: ElementType,
    pub multiplicity: Multiplicity,
}

pub struct Continuity {
    pub input: usize,
    pub output: usize,
}
```

A valid continuity declaration is one-to-one and partial. Each referenced input and output port is
linear, their element types agree, and no port occurs in more than one continuity pair. An input
without a continuity edge is consumed. An output without a continuity edge starts a new resource
trajectory.

The initial registry contains:

```text
qalloc:   ()             -> qubit          continuity: none
h:        qubit          -> qubit          continuity: input 0 -> output 0
cx:       qubit, qubit   -> qubit, qubit   continuity: 0 -> 0, 1 -> 1
measure:  qubit          -> bool           continuity: none
discard:  qubit          -> ()             continuity: none
```

`qalloc` prepares `|0>`. Other initial states are expressed by following it with gates. `measure`
consumes its qubit. A future operation with a post-measurement qubit uses a different name and an
explicit continuity edge rather than context-dependent behavior.

Initially, operation signatures and HUGR implementations live behind a compiler registry. The API
must not make the registry's built-in nature observable to type or resource analysis. A later
resolver may populate the same representation from HUGR extension metadata.

## Logical Resource Flow

Logical resource flow is determined by the program, not by the schedule.

For a call statement with iteration domain `D_s`, let:

```text
A_p : D_s -> D_input
```

be the access relation for input port `p`, and let:

```text
O_q : D_s -> D_output
```

be the definition relation for output port `q`. For a continuity declaration `p -> q`, the
successor relation is:

```text
F_pq = reverse(A_p) compose O_q
```

It maps each consumed logical value point to the logical value point that continues the same
resource.

The existing exact-once linear analysis supplies the foundational invariants:

- every linear logical value has one producer;
- every linear logical value has one consumer or is exported at the system boundary;
- no access broadcasts one linear value to multiple call instances;
- distinct active accesses do not overlap;
- definitions are injective, disjoint, and complete.

Continuity validation adds these invariants:

- each logical point has at most one continuity predecessor and successor;
- a system input or operation output without a predecessor starts a trajectory;
- a system output or consuming operation input without a successor ends a trajectory;
- continuity never changes element type.

Consequently, qubit resource flow consists of non-branching trajectories. A two-qubit gate extends
two trajectories; it does not merge them merely because both wires participate in one operation.

The scheduler must preserve producer-consumer dependencies induced by calls and ordinary
expressions. It does not choose which consumer receives a resource and cannot change continuity.

## Scheduled IR

The ISL AST is translated into an owned representation before either backend runs:

```rust
pub enum ScheduledNode {
    Loop {
        iterator: IndexVar,
        init: IndexExpr,
        condition: Predicate,
        step: IndexExpr,
        body: Box<ScheduledNode>,
    },
    If {
        condition: Predicate,
        then_body: Box<ScheduledNode>,
        else_body: Option<Box<ScheduledNode>>,
    },
    Sequence(Vec<ScheduledNode>),
    Invoke {
        statement: StatementId,
        indices: Vec<IndexExpr>,
    },
}
```

`IndexExpr` and `Predicate` are typed internal expression trees, not strings containing C. They
cover the ISL AST operations needed by existing scheduled-codegen fixtures, including integer
constants, identifiers, arithmetic, comparisons, Boolean combinations, division rounding, and
min/max where generated by ISL. Unsupported ISL expression kinds produce a diagnostic at
translation time.

`Invoke` refers to a resolved statement ID rather than dispatching by a string in each backend.
The statement table contains ordinary definitions, reduction initialization/steps, and typed call
statements.

`ScheduledC` is migrated to consume this IR. Existing C snapshots and end-to-end tests establish
that the extraction does not alter loop structure or semantics. HUGR consumes the same control
structure but performs its own value realization.

The `Predicate` representation is intentionally extensible to runtime value conditions. The first
implementation constructs only affine predicates obtained from ISL.

## Parameter Specialization

Alpha analysis and schedule legality remain symbolic. HUGR generation requires concrete bindings
for every parameter that affects a domain, loop bound, array shape, or realization:

```text
generate_hugr(system, schedule, {T: 10, N: 32})
```

Generation performs these steps:

1. Validate the target mapping symbolically against the normalized statement set.
2. Validate all producer-consumer dependences symbolically.
3. Check that required parameter bindings are present and satisfy the system parameter domain.
4. Restrict domains and the schedule to the supplied parameter point.
5. Generate the ISL AST and lower it to scheduled IR.
6. Infer a concrete realization and HUGR types.

Specialization avoids representing one Alpha parameter simultaneously as an unrelated HUGR type
argument and runtime integer. Parametric HUGR generation is deferred explicitly below.

## Compact Quantum Realization

A logical domain is not a storage declaration. Realization maps logical value points to abstract
resource lanes after a legal schedule and concrete parameters are known.

For:

```alpha
linear Q : {[t,i] : 0 <= t < T and 0 <= i < N} of qubit;
```

with continuity flow:

```text
Q[0,i] -> Q[1,i] -> ... -> Q[T-1,i]
```

the desired realization is:

```text
Q[t,i] -> lane[i]
```

The initial realization algorithm assigns a distinct lane to every trajectory root and propagates
that lane through continuity relations. It does not reuse a lane for two unrelated trajectories.
This already removes logical version dimensions without requiring interference-graph coloring.

A successful realization proves:

- every logical qubit point maps to exactly one lane;
- continuity predecessors and successors map to the same lane;
- simultaneously used gate operands map to distinct lanes;
- the root-to-lane mapping is injective;
- each emitted container group has a concrete rectangular shape;
- each group has a uniform boundary state, or can be partitioned into rectangular groups that do.

Root kind and sink kind matter. System-input trajectories begin occupied; `qalloc` trajectories
begin empty and become occupied at allocation. System-output trajectories end occupied;
`measure` and `discard` trajectories end empty. If mixed roots or sinks cannot be partitioned into
supported rectangular groups, realization fails rather than inventing a dense fallback.

The realization result is backend-neutral metadata conceptually equivalent to:

```rust
pub struct Realization {
    pub groups: Vec<ResourceGroup>,
    pub logical_to_lane: Vec<LogicalLaneMap>,
}
```

Each logical-to-lane map is an exact ISL relation plus a concrete group ID. The first implementation
may restrict accepted maps to affine projections and permutations needed by rectangular examples.

### No dense quantum fallback

If compact realization cannot be proved, HUGR generation fails. It must never silently allocate
one qubit for every point in the logical domain. Dense materialization remains a valid conservative
strategy for classical C storage, but it is too surprising and potentially enormous for quantum
resources.

## HUGR State Representation

A plain HUGR array cannot support dynamic pointwise quantum updates: array `get` is restricted to
copyable elements, and linear `set` exchanges an existing element rather than leaving a slot empty.
The HUGR `collections.borrow_arr` extension directly represents the required occupancy state.

Each rectangular quantum resource group becomes:

```text
borrow_array<size, qubit>
```

The relevant operations are:

```text
borrow(array, index)         -> array-with-empty-slot, value
return(array, index, value)  -> array-with-filled-slot
new_all_borrowed()           -> entirely empty array
discard_all_borrowed(array)  -> ()
```

They lower gates as follows:

```text
h at i:
    state, q = borrow(state, i)
    q1 = h(q)
    state = return(state, i, q1)

cx at i,j:
    state, q = borrow(state, i)
    state, r = borrow(state, j)
    q1, r1 = cx(q, r)
    state = return(state, i, q1)
    state = return(state, j, r1)

qalloc at i:
    q = qalloc()
    state = return(state, i, q)

measure at i:
    state, q = borrow(state, i)
    bit = measure(q)
    // The lane remains empty.

discard at i:
    state, q = borrow(state, i)
    discard(q)
    // The lane remains empty.
```

Borrow-array operations may panic when their occupancy preconditions are violated. Alpha must
prove those preconditions statically from exact-once flow, continuity, lane injectivity, and
schedule legality. Generated HUGRs do not add dynamic occupancy checks.

At boundaries:

- an incoming ordinary HUGR array converts to a fully occupied borrow array;
- an allocation-root group starts with `new_all_borrowed`;
- a fully occupied output group converts back to an ordinary HUGR array;
- a fully consumed group is discharged with `discard_all_borrowed`.

Write-once classical result collections, including measurement results, may similarly use an empty
borrow array during construction and convert to an ordinary array after completeness is proved.
Copyable classical inputs may remain ordinary arrays.

## Structured HUGR Lowering

The HUGR backend walks scheduled IR:

- `Loop` becomes a HUGR `TailLoop`.
- `If` becomes a HUGR `Conditional`.
- `Sequence` emits its children in order.
- `Invoke` evaluates index expressions, accesses realized containers, and emits the resolved
  operation or ordinary Alpha computation.

A loop's `rest` row contains every realized linear container and classical value live across its
back edge, plus loop-control values required by the generated condition and step. Scheduled
liveness determines this row; it does not change logical flow or lane assignment.

A conditional passes the required state into each branch. Both branches must return the same HUGR
type row. Occupancy may differ internally, but any later borrow or boundary conversion must be
proved valid on every reachable branch.

The backend lowers `IndexExpr` and affine `Predicate` nodes to HUGR integer arithmetic and
comparison operations. It never parses C text or invokes a C compiler as an intermediate step.

The finished HUGR is validated with HUGR's validator before being returned or serialized.

## Schedule And Occupancy Legality

Existing schedule validation remains responsible for statement coverage, totality, injectivity,
shared schedule width, and known statement names. Dependence legality is extended so typed call
inputs read their referenced logical values and call outputs produce their targets.

For the initial no-cross-trajectory-reuse realization, occupancy safety follows from these exact
properties:

- a system-input root is occupied at entry;
- a `qalloc` root is returned once into its unique initially empty lane;
- every continuity operation borrows an occupied predecessor and returns its successor atomically
  to the same lane;
- every consuming sink borrows once and never returns to that trajectory;
- every producer is scheduled before its consumer;
- distinct roots map to distinct lanes.

The compiler verifies these properties over the relational flow graph and realized lane maps. It
also verifies that each structured loop and conditional carries all live containers required by
later invocations. A failure is a realization or scheduling diagnostic, not a HUGR runtime check.

## Diagnostics

Diagnostics belong to the phase that owns the failed invariant.

Type and resource analysis reports:

- a qubit variable not declared `linear`;
- an operation input or output type mismatch;
- an operation port multiplicity mismatch;
- an invalid continuity signature;
- a linear call input or output that lacks an exact affine access relation;
- aliased linear operands within one call;
- existing exact-once definition, overlap, broadcast, or unconsumed-resource errors.

Scheduling reports:

- missing, duplicate, unknown, partial, or non-injective statement mappings;
- incompatible schedule widths;
- a producer not scheduled strictly before its consumer.

Specialization and realization report:

- missing or invalid parameter bindings;
- unsupported non-rectangular physical domains;
- a logical point with no unique trajectory lane;
- a continuity edge whose endpoints map to different lanes;
- aliased gate operands after realization;
- unsupported mixed boundary occupancy;
- a borrow, return, conversion, or discard whose occupancy cannot be proved;
- failure to derive a compact realization, explicitly noting that dense quantum fallback is not
  performed.

HUGR lowering reports:

- an operation without a registered HUGR implementation;
- an index expression or predicate without HUGR lowering;
- HUGR builder errors;
- final HUGR validation errors.

Diagnostics should retain statement names and exact ISL witness sets or relations where available.

## Deferred Capabilities

### Irregular domain packing

Alpha semantics continue to permit arbitrary polyhedral domains. The initial HUGR backend accepts
only resource groups with a proved rectangular physical shape and affine lane map. Irregular
checkerboards, triangular domains, and unions that cannot be partitioned into supported rectangles
produce a dedicated realization diagnostic.

Future support should add explicit packing maps and exact rank/cardinality machinery. This may use
Barvinok or another exact Presburger ranking implementation. The logical resource-flow model and
scheduled IR do not change.

### Parametric HUGR generation

The first backend specializes all shape- and control-affecting Alpha parameters. Future parametric
emission may represent Alpha parameters as HUGR type parameters, runtime values, or a proven
combination of both. It must define how a type-level array length and runtime loop bound are known
to denote the same Alpha parameter; it must not rely on an undocumented caller convention.

### Measurement-dependent control

The initial backend may produce and return measurement results, but a runtime measurement result
cannot control whether a gate call executes. ISL affine guards over parameters and loop indices are
supported.

Future measurement-based feed-forward requires:

- a source-level conditional call or statement construct;
- scheduled-IR conditions that reference runtime values;
- branch-sensitive linear resource summaries;
- proof that both branches return compatible realized state rows;
- HUGR `Conditional` lowering for the runtime condition.

`ScheduledNode::If` and its typed condition interface must remain extensible to this case. The first
implementation must issue an unsupported-feature diagnostic rather than assigning accidental
semantics to a measurement-controlled gate.

### Cross-trajectory resource reuse

The initial realization gives each trajectory root a distinct lane. A later allocator may reuse a
lane between unrelated trajectories whose scheduled lifetimes do not overlap. That pass requires
schedule-dependent liveness, an interference relation, and coloring or another allocation policy.
It is separate from continuity, which remains schedule-independent.

### Imported HUGR operations

The initial operation registry is built into the compiler. Future work may load typed operation
signatures and implementations from HUGR extension registries. Imported metadata must provide or
be augmented with Alpha continuity information. If HUGR metadata cannot express continuity, Alpha
may add an external annotation format, but no source syntax is committed by this design.

### Source-level `inout`

Explicit linear inputs and outputs are sufficient for the initial design. A later `inout` feature
may abbreviate a paired input/output contract, but it must lower to explicit ports and must not
introduce mutable variables, implicit versioning, or schedule-dependent dataflow.

## Testing Strategy

Tests are layered so each abstraction can fail independently.

1. Syntax tests cover element types, call equations, zero/multiple outputs, and recovery.
2. Model tests cover mandatory linear qubits, operation type checking, port multiplicity, affine
   access requirements, and continuity validation.
3. Relational tests cover gate input/output relations, roots, sinks, exact-once flow, aliasing, and
   `Q[t,i] -> Q[t+1,i]` trajectories.
4. Scheduled-IR snapshots cover loops, affine guards, statement calls, skewed/permuted schedules,
   and every supported ISL expression operator.
5. Existing ScheduledC snapshots and executable tests prove that migrating to scheduled IR does
   not change C behavior.
6. Specialization tests cover valid points, missing parameters, and parameter-domain violations.
7. Realization tests prove `Q[t,i] -> lane[i]`, rectangular grouping, uniform boundary occupancy,
   and clear rejection of irregular or unprovable layouts.
8. HUGR unit tests inspect signatures, `TailLoop`, `Conditional`, borrow-array operations, quantum
   operations, and boundary conversions.
9. End-to-end tests generate HUGRs under multiple legal schedules, run HUGR validation, and show
   that illegal schedules fail before emission.
10. Deferred-feature tests pin diagnostics for irregular domains, parametric generation, and
    measurement-dependent gate control.

## Implementation Sequence

1. Add element types to syntax, model, and transform IR; require explicit linearity for qubits.
2. Generalize call resolution and add the backend-independent operation registry.
3. Add call statement extraction and relational continuity/resource-flow analysis.
4. Introduce typed scheduled IR, lower the ISL AST into it, and migrate ScheduledC.
5. Add concrete parameter specialization.
6. Add trajectory extraction, rectangular grouping, and logical-to-lane realization.
7. Add occupancy and structured liveness verification.
8. Add HUGR dependencies and lower scheduled IR through borrow arrays and registered operations.
9. Expose scheduled HUGR generation through Rust, CLI, and Python APIs.
10. Add executable examples and document all initial restrictions and deferred capabilities.

Each stage must have focused tests before the next stage depends on it. The HUGR backend must not
be used as the test oracle for resource-flow or realization correctness; those layers expose and
test their own backend-neutral results.
