# Linear Variables in Alpha

## Goal

Add a general notion of linear variables to Alpha without coupling it to qubits or any other
element type. A linear declaration makes every point in the variable's polyhedral domain a
resource that must be consumed exactly once. Existing variables remain unrestricted by default.

The design separates two concerns:

1. Multiplicity analysis prevents a linear value from becoming unrestricted without an explicit
   consuming operation.
2. Polyhedral resource analysis proves exact pointwise use over parameterized array domains.

Scheduling remains downstream. It may reorder statement instances subject to the existing data
dependences without changing whether a program is linear.

## Non-goals

- Defining qubits or quantum operations.
- Adding a general scalar type system.
- Inferring whether a declaration should be linear.
- Inferring multiplicity signatures for opaque external functions.
- Making resource correctness depend on source statement order.
- Supporting approximate resource analysis. Unsupported constructs produce diagnostics instead
  of silently weakening the check.

## Surface Syntax

`linear` is a modifier on a variable clause:

```alpha
affine transfer [N] -> {: N > 0}
    inputs
        linear X, Y : {[i] : 0 <= i < N};
        A : {[i] : 0 <= i < N};
    outputs
        linear Z : {[i] : 0 <= i < N};
    locals
        linear T : {[i] : 0 <= i < N};
    let
        T[i] = X[i];
        Z[i] = T[i];
.
```

The modifier applies to the entire comma group. Omitting it means `unrestricted`, preserving all
existing programs. `linear` is independent of the variable's domain and any future element type.

Conceptually, the grammar changes from:

```text
variable-clause := (IDENT ',')* IDENT ':' domain ';'?
```

to:

```text
variable-clause := 'linear'? (IDENT ',')* 'fuzzy'? IDENT ':' domain ('->' domain)? ';'?
```

`linear` becomes a reserved keyword. The lossless CST records the prefix modifier on the first
variable node in the comma group. The semantic resolver carries it through the pending names to
the terminating declaration, then applies the resolved multiplicity to every variable in the
group, alongside the group's inherited domain. Parser lookahead consumes an initial `linear`
before applying the existing fuzzy-clause lookahead; therefore `linear X, fuzzy Y : D -> R` is one
linear fuzzy group. `linear` after the first name is invalid.

## Semantic Representation

Multiplicity is a separate axis from value type:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Multiplicity {
    Linear,
    #[default]
    Unrestricted,
}

pub struct VariableInfo {
    pub domain: isl::Set,
    pub multiplicity: Multiplicity,
}
```

The implementation may initially add `multiplicity` to the existing resolved variable data
rather than introduce `VariableInfo` literally. The important invariant is that all later phases
query one resolved value by variable identity; they must not repeatedly inspect syntax.

Variable identity is fixed in this increment:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VariableId(u32);
```

`Resolver::new` assigns IDs monotonically in source order within a system and stores both
name-to-ID and ID-to-declaration information. IDs are meaningful only together with their owning
system. Analysis maps, including `LinearUses`, use `VariableId`; user-facing diagnostics recover
the source name from declaration information. The lowered transform IR retains names for printing
and code generation and does not use `VariableId` as a cross-system identifier.

Use an enum rather than a Boolean. It names the semantic concept and permits a later extension to
other multiplicities without changing every consumer. Do not represent linearity as a special
element type or as an `owned` flag.

Assignment compatibility is deliberately explicit rather than expressed through a subtype order:

- unrestricted expression -> unrestricted target: allowed;
- unrestricted expression -> linear target: allowed, after which the target is used exactly once;
- linear expression -> linear target: allowed;
- linear expression -> unrestricted target: rejected unless an explicit operation's signature
  has already consumed the linear input and produced an unrestricted result.

## Analysis Placement

Linearity belongs in `alpha-model`, after expression/context-domain inference and before lowering
to `alpha-transform`.

Domain inference supplies the exact consumer domains needed for restrictions, cases, dependence
expressions, reductions, and system bodies. Running before lowering preserves source spans for IDE
diagnostics and guarantees that invalid programs never enter transformation or code generation.

The per-system pipeline becomes:

1. Resolve names, declarations, domains, and multiplicities.
2. Infer expression and context domains.
3. Check uniqueness and existing well-formedness conditions.
4. Infer expression multiplicities and collect linear uses.
5. Check pointwise linear resource accounting.
6. Lower only a diagnostic-free system.

Steps 4 and 5 are separate APIs even if `analyze_system` invokes them together.

## Multiplicity Analysis

Every expression receives a summary:

```rust
pub struct ExprFacts {
    pub result: Multiplicity,
    pub uses: LinearUses,
}
```

`LinearUses` maps a declared linear variable to one or more source-located access relations. It is
an analysis value, not part of the persistent syntax tree.

### Structural expressions

- A literal or index value is unrestricted and has no linear uses.
- A reference to an unrestricted variable is unrestricted and has no linear use entry.
- A reference to a linear variable is linear and records one use.
- Dependence/indexing composes the enclosing consumer domain with the access function.
- Restriction intersects the enclosing consumer domain with the restriction domain.
- `auto` uses the inferred context domain already computed for its case branch.
- Parentheses and other transparent forms preserve both result multiplicity and uses.

A direct structural transfer such as `T[i] = X[i]` therefore consumes the selected point of `X`
and produces the corresponding point of linear target `T`.

### Operators

Each operator has a multiplicity signature with one multiplicity per input and output port. A
linear input port consumes its argument. An unrestricted input port rejects a linear argument,
because an unrestricted implementation may copy or discard it. Output multiplicity determines the
result multiplicity independently of the inputs.

All existing built-in operators initially have unrestricted inputs and unrestricted output. This
keeps their current meaning and rejects their use with linear operands until an explicit, reviewed
signature is added. Signatures live in `alpha-model::multiplicity` beside expression analysis, not
as special cases in the resource checker:

```rust
pub struct PortSignature {
  pub inputs: Vec<Multiplicity>,
  pub outputs: Vec<Multiplicity>,
}

pub fn builtin_signature(operator: BuiltinOperator, arity: usize) -> PortSignature;
```

The initial registry returns unrestricted inputs and outputs for every existing built-in. Operator
classification remains in its existing owning module; it supplies the classified operator and
arity to this registry rather than duplicating token-string matching in multiplicity analysis.

The existing declaration `external f(n)` retains its current behavior and means `n` unrestricted
inputs and one unrestricted output. A later syntax extension may expose explicit port signatures,
for example:

```alpha
external move(linear) -> linear
external combine(linear, linear) -> linear
external observe(linear) -> unrestricted
external destroy(linear) -> ()
```

The semantic representation must support these signatures now, but parsing this extended external
syntax is a separate implementation increment. Until that increment lands, source programs can
exercise general linear transfer through equations but cannot pass linear values to opaque calls.
An existing external declaration therefore resolves to an explicit all-unrestricted
`PortSignature`, never to a missing or inferred signature.

System calls use the multiplicities of the callee system's declared inputs and outputs. The
existing multi-input/multi-output `UseEquation` is the call boundary. Each input expression is
checked against the corresponding input port, and each output expression against the corresponding
output port. Arity errors remain distinct from multiplicity errors.

### Assignment

A standard equation performs two checks:

1. Its right-hand expression may not widen `Linear` to an unrestricted target.
2. A complete definition produces every point in a linear target's declared domain exactly once.

Alpha's existing single-assignment and completeness checks provide the producer-side guarantee for
standard equations. Resource analysis nevertheless represents production explicitly for both
equation forms. Each definition contributes a relation from defining statement instances to points
of its target variable. For every linear local or output, definition relations must be injective,
pairwise range-disjoint, and jointly cover the target's declared domain.

For a use-equation output, the statement instances are the product of its resolved instantiation
domain and subsystem dimensions. The output expression maps those instances to target points. A
linear output position is initially accepted only when that expression reduces to a dependence
over one declared linear variable. Its resulting definition relation participates in the same
injectivity, disjointness, and coverage checks as standard equations. A non-affine or otherwise
unresolved output mapping produces `LinearityUnsupportedHere`, and missing target points produce
`LinearDefinitionIncomplete` with the uncovered set.

## Pointwise Resource Accounting

Linearity applies to array elements, not merely variable names. For a linear variable `X` with
domain `D_X`, every occurrence contributes an exact ISL relation:

```text
R_i : consumer points -> points of X
```

For all uses of `X`, require:

1. **Injectivity:** each `R_i` is injective. One source point cannot be consumed by multiple
   consumer instances through a broadcast or many-to-one access.
2. **Disjointness:** `range(R_i) intersect range(R_j)` is empty for every distinct pair of
   simultaneously active uses.
3. **Coverage:** the union of all consumed ranges equals `D_X` after accounting for control-flow
   alternatives and the system boundary.

Inputs begin as available resources. Complete definitions create local and output resources.
Exporting a declared linear output contributes a boundary consumption over its full domain. There
is no implicit drop and no implicit return of an unconsumed input.

The checker uses exact ISL operations only. It must not use an operation documented as an
over-approximation. Witness sets are retained in diagnostics so parameterized errors remain useful.

## Control Flow

Alpha has two materially different forms of branching.

### Domain-partitioned `case`

Existing analysis proves case branch domains disjoint and complete. The linear-use summary is the
union of each branch's domain-restricted relations. Reads of the same variable in distinct,
disjoint branch domains therefore do not conflict.

### Runtime `if`

Both reachable runtime branches are alternatives for the same consumer points. They must have equal linear
use summaries: the same variables, access relations, and resource footprints after canonical ISL
comparison. That footprint is counted once. Uses in the condition are counted in addition to the
selected branch footprint. A branch whose inferred context domain is statically empty contributes
no uses and does not participate in the equality check.

This is conservative but schedule-neutral. It rejects data-dependent resource behavior that the
polyhedral model cannot express, while allowing either branch to compute a different unrestricted
result from the same consumed resources.

### Reductions and index-changing forms

In the initial implementation, a linear reference beneath reduction, select, convolution, or any
other index-changing form not listed under structural expressions produces
`LinearityUnsupportedHere`. No such subtree is traversed as if it used the enclosing context.

A later increment may support one construct when it can derive a relation whose domain is exactly
the set of dynamic body instances that evaluate the reference and whose range is exactly the
referenced variable points selected by those instances. The general injectivity, disjointness, and
coverage checks then decide legality; there is no permanent lexical rule that bans reduction.

An unimplemented relation derivation produces `LinearityUnsupportedHere`. It must never omit a
subtree from resource accounting.

## Schedule Interaction

Linearity is a property of declarations and access relations, not timestamps. A legal schedule may
reorder, tile, fuse, split, or parallelize statement instances while preserving:

- each statement's domain and access relations;
- total and injective scheduling of each statement;
- existing producer-to-consumer dependences.

For a linear variable, exact-use conditions such as

```text
range(union R_i) = D_X
```

and pairwise range disjointness are invariant under such a rescheduling. Since Alpha is
single-assignment, linearity introduces no write-after-read or write-after-write dependencies. A
linear read is already an ordinary read-after-write data dependence.

The scheduler does not need to inspect multiplicity. Transformations must preserve multiplicity
metadata and access relations, and the existing legality checker continues to enforce dataflow
order.

## Diagnostics

Add source-located diagnostics with witness domains where applicable:

- `LinearValueWidened`: a linear expression is assigned to an unrestricted target or port.
- `LinearUseNotInjective`: one occurrence consumes some resource points more than once.
- `LinearUsesOverlap`: two simultaneously active occurrences consume overlapping points.
- `LinearValueUnconsumed`: some points in a linear variable's domain have no consumer.
- `LinearBranchMismatch`: runtime branches consume different linear resources.
- `LinearArgumentToUnrestrictedPort`: an opaque operation could copy or discard a linear value.
- `LinearDefinitionIncomplete`: a use-equation does not produce every point of a linear target
  exactly once.
- `LinearityUnsupportedHere`: an exact access relation is unavailable for a construct.

Diagnostics should name the declaration and offending use sites. Overlap and coverage errors should
render the exact offending ISL set; this set is itself the symbolic witness and does not require
sampling a concrete point. If rendering fails, report the variable and relation sites without
hiding the primary error. One unsupported construct should produce one primary diagnostic rather
than cascaded coverage errors for the same variable.

## Architecture Boundaries

Add a focused multiplicity module in `alpha-model` rather than extending domain inference itself.
The expected responsibilities are:

- `alpha-syntax`: parse and expose the `linear` declaration modifier.
- `alpha-model::multiplicity`: resolve declaration multiplicities, infer expression summaries,
  derive exact access relations, and emit linearity diagnostics.
- `alpha-model::analyze`: invoke multiplicity analysis after domain/context inference.
- `alpha-transform::ir`: carry resolved multiplicity on variables so transformations and future
  backends preserve it; do not re-check source-level linearity here.
- `alpha-codegen`: no new scheduling policy; reject an internal inconsistency if linear metadata
  reaches a backend path that cannot preserve it.

Variable identity is the stable per-system `VariableId` assigned during resolution, not raw text.

## Delivery Increments

1. Parse `linear` and resolve multiplicity for variable groups; carry it into the resolved IR.
2. Implement structural multiplicity inference for literals, references, dependence, restriction,
   case, runtime `if`, and standard assignment.
3. Collect exact use relations and check injectivity, overlap, and coverage for standard equations.
4. Add system-call port checking and exact production through use equations.
5. Add explicit external multiplicity-signature syntax and consuming primitives.
6. Extend exact relation derivation to reductions and remaining index-changing forms.

Each increment keeps existing unrestricted programs valid. An increment may reject an unsupported
linear construct, but it may not accept one without accounting for every reachable linear use.

## Testing

Tests are layered by ownership boundary:

- Parser tests: modifier placement, comma-group inheritance, lossless round-trip, recovery, and
  compatibility with the fixture corpus.
- Model tests: default unrestricted multiplicity, resolved linear groups, transfer compatibility,
  rejected widening, and operator-port checking.
- Polyhedral tests: identity use, affine permutation, broadcast/non-injective use, overlapping
  reads, partial coverage, disjoint case partitions, and parameterized witness sets.
- Control-flow tests: equal runtime branch summaries pass; differing variables, relations, or
  domains fail.
- Call tests: system input/output port matching, multiple linear outputs, unrestricted legacy
  externals, and incomplete use-equation production.
- Regression tests: existing unrestricted fixtures retain zero new diagnostics; existing schedule
  legality tests are unchanged.

Property-oriented tests should verify that applying different legal schedules does not change the
linearity result for the same resolved system.

## Success Criteria

- Existing source without `linear` has unchanged semantics and diagnostics.
- Every point of every accepted linear declaration has exactly one statically proven consumer,
  including output-boundary consumption.
- A linear value cannot be laundered through an unrestricted variable or opaque operation.
- Runtime alternatives are accepted only when their linear effects agree.
- Unsupported constructs fail explicitly.
- Schedule legality and optimization remain based on data dependences, with no source-order
  constraint added by linearity.
- No part of the representation or analysis refers to qubits.