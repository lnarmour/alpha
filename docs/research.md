# Research notes — Alpha and quantum error correction

Running, informal doc. Captures exploratory thinking on whether/how the Alpha polyhedral
toolchain (this repo) could apply to quantum error correction (QEC), motivated by needing compact
representations for reasoning about large programs over arrays of qubits — exactly the class of
problem the polyhedral model targets for classical array computations. Append to this rather than
rewriting history; each entry is a snapshot of thinking at a point in time, not a spec.

## 2026-08-08 — initial brainstorm

### QEC background (for orientation)

- Physical qubits are noisy (~10⁻³ error rates); QEC encodes one *logical* qubit across many
  *physical* qubits so errors can be detected via syndrome measurement (without collapsing the
  encoded state) and corrected faster than they accumulate. Threshold theorem: below some physical
  error rate (~1% for surface codes), increasing code distance suppresses logical error
  exponentially.
- Code families: **surface codes** (2D nearest-neighbor, high threshold, ~1000+ physical qubits
  per logical qubit — the practical default today), **qLDPC codes** (e.g. IBM's bivariate bicycle
  codes, trade locality for ~10x lower overhead), **bosonic/cat codes** (redundancy in a single
  oscillator mode, biases the noise channel).
- "Quantum compiler" for QEC spans several layers: logical→physical mapping (code choice,
  distance, layout), fault-tolerant gate synthesis (not all gates are transversal under a given
  code — e.g. surface-code T-gates need magic-state distillation, which dominates resource cost),
  routing/scheduling under hardware connectivity (lattice surgery = geometric merge/split of
  surface-code patches), and real-time classical decoding (MWPM, union-find, ML decoders — must
  keep pace with the physical gate clock).
- Relevant tooling landscape: Qiskit transpiler, Cirq/Stim (Stim = fast stabilizer-circuit/QEC
  simulation, a de facto interchange format for decoding research), PyMatching (decoding),
  Quantinuum's HUGR/tket2/Guppy stack (compiler IR, not a simulation format — see below), Lattice
  Surgery Compiler and other academic FT compilers.

### Where Alpha's model actually resonates

Alpha's heritage is systolic-array synthesis via uniform/affine recurrence equations (SAREs) over
parametric polyhedral domains, with an explicit normalize → schedule → generate pipeline (see
[`design.md`](design.md)). Several QEC problems have the same shape:

1. **Syndrome extraction as a stencil** (strongest fit, lowest effort). Stabilizer measurement on
   a surface code is a spatial stencil (each stabilizer touches its 4 neighbor qubits) repeated
   over time — structurally the same as the Jacobi/heat-stencil kernels polyhedral compilers
   already target (cf. Polybench). Could express `Syndrome[x,y,t]` as an affine recurrence over a
   parametric `[d,T]->{: d>0, T>0}` domain referencing `Qubit[x±1,y,t]`/`Qubit[x,y±1,t]`. Demo
   payoff: one `.alpha` source parametric in code distance `d` generates a correctly-scheduled
   instruction sequence for any `d` via `ScheduledC`-style codegen, instead of hand-unrolling per
   distance — the "compact representation" pitch made concrete.

2. **Lattice-surgery scheduling via existing legality machinery.** Merge/split operations on an
   array of surface-code patches have real scheduling constraints (two merges can't share an
   ancilla region in the same timestep). Same *shape* of problem as the array-reuse/dependence
   legality checking [`alpha-codegen/src/legality.rs`](../alpha-codegen/src/legality.rs) already
   does for classical array programs. Modeling patches × timesteps as an affine domain and
   pointing that existing checker at it (rather than writing new analysis) is a small, high-signal
   PoC.

3. **New backend: HUGR, not a new frontend.** `alpha-codegen` already separates scheduled IR from
   the C emitter (`writec.rs` vs `scheduledc.rs`/`simplec.rs`). A HUGR backend fits this seam:
   HUGR (Quantinuum's Hierarchical Unified Graph Representation, underlying `tket2`/Guppy) is a
   hierarchical dataflow-graph IR — nodes can contain nested child regions, which maps directly
   onto the loop-nest structure Alpha already schedules — with linear-typed wires for qubits
   (structurally enforces no-cloning) and explicit control-flow node kinds (conditional,
   tail-loop). Preferred over Stim as a target: Stim is a flat stabilizer-circuit *simulation*
   format, not designed to receive output from an arbitrary optimizing frontend; HUGR is an actual
   compiler IR meant to be a lowering target, and it's Rust-native (`hugr` crate on crates.io),
   fitting `alpha-rs` better than emitting text. It also gives a cleaner home for the "doesn't fit"
   boundary below: decoder feedback / mid-circuit branching could be represented as an opaque
   HUGR conditional/tail-loop node sitting in the same graph as the affine-generated
   stabilizer-extraction subgraph, rather than punting to a wholly separate ad hoc format.

4. **Magic-state distillation factories as pipelines** (weaker candidate). Multi-level distillation
   is a pipelined/systolic reduction (many noisy inputs → fewer higher-fidelity outputs per level),
   close to Alpha's original systolic-array target domain — more of a "this also fits" argument
   than a compelling standalone demo.

### Where it doesn't fit

Decoders (MWPM/union-find) and anything with classically-controlled branching on mid-circuit
measurement outcomes are not affine/static control — Alpha's scheduler can't derive them. A HUGR
backend can at least *represent* that boundary cleanly (see above), but the dynamic part still has
to be supplied from outside Alpha, not generated by it. Also: `alpha-rs` explicitly puts automatic
schedule *search* out of scope (per [`design.md`](design.md)) — schedules are supplied, not
derived — so any PoC needs a hand-written schedule (as `ScheduledC`'s notebook workflow already
expects), not an expectation that Alpha discovers a good lattice-surgery ordering on its own.

### Leaning toward, next step

Option 1 (syndrome-extraction stencil) is the cleanest starting point: same polyhedral techniques,
new domain, zero new machinery — just a new `.alpha` fixture and a hand-supplied schedule. Option 3
(HUGR backend) is the natural follow-on once there's a scheduled IR worth emitting somewhere more
consequential than C. Next step if pursued: sketch the actual Alpha syntax for a small
surface-code syndrome-extraction program and see whether the existing `Normalize`/`ScheduledC`
pipeline handles it as-is.
