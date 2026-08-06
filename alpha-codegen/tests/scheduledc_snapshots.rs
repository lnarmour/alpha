//! Tier 3/4 of `docs/codegen-test-design.md`'s test matrix — black-box, public-API-only
//! (`alpha_codegen::generate_scheduled_system`/`generate_system`/`describe_normalized_system`)
//! snapshots of the *loop nest* `ScheduledC` emits under a real schedule. This is the only tier in
//! the whole matrix where loop structure exists at all: `ScheduledC` builds the entire program as
//! one `AstBuild::generate` call (§8.1 of `docs/scheduled-codegen-design.md`), so a single
//! statement's own body codegen never sees a loop — only the whole-program AST walk does.
//!
//! Every fixture here is duplicated from `alpha-codegen/src/test_util.rs` (that module is
//! `pub(crate)`, not visible from an external integration-test crate) — kept in sync by the
//! `_scratch_fixture_check.rs`-style verification already done once while designing this suite;
//! see `docs/codegen-test-design.md` §4.

use alpha_model::Resolver;
use std::process::Command;

const PREFIX_SUM: &str = "affine PrefixSum [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
.";

const INT_LITERAL: &str = "affine IntLiteral [N]->{:N>0}
    outputs Y: [N]
    let Y[i] = 3[];
.";

const PLAIN_COPY: &str = "affine Copy [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = X[i];
.";

const IF_THEN_ELSE: &str = "affine IfThenElse [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = if (X[i] > 0[]) then X[i] else -(X[i]);
.";

// Deliberately two fully-explicit branches, no `auto` — see test_util.rs's own copy of this
// fixture for why: an `auto`-default version of this same shape was found to mis-generate (the
// reduce's own domain came out unrestricted, silently summing the whole array every iteration).
const REDUCE_CASE_BODY: &str = "affine ReduceCaseBody [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: case {
        {:j=i}: X[j] + X[j];
        {:j<i}: X[j];
    });
.";

// -------------------------------------------------------------------------------------------
// §5.5 fixtures: two 1D outputs, dataflow x schedule combos.
// -------------------------------------------------------------------------------------------

const TWO_INDEPENDENT: &str = "affine TwoIndependent [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        W: [N]
    let
        Y[i] = X[i];
        W[i] = -(X[i]);
.";

const PRODUCER_CONSUMER_NO_REDUCE: &str = "affine ProducerConsumer [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        W: [N]
    let
        Y[i] = X[i];
        W[i] = Y[i] + 1[];
.";

const PREFIX_SUM_WITH_CONSUMER: &str = "affine PrefixSumConsumer [N]->{:N>0}
    inputs  X: [N]
    outputs Z: [N]
    locals  Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
        Z[i] = Y[i] + 1[];
.";

const TWO_REDUCES_FAN_OUT: &str = "affine TwoReducesFanOut [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        MaxY: [N]
    let
        Y[i] = reduce(+, [j], {:j<=i}: X[j]);
        MaxY[i] = reduce(max, [j], {:j<=i}: X[j]);
.";

const NESTED_REDUCE_DEPENDENCY: &str = "affine NestedReduceDependency [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
        Z: [N]
    let
        Y[i] = reduce(+, [j], {:j<=i}: X[j]);
        Z[i] = reduce(+, [j], {:j<=i}: Y[j]);
.";

// §5.6: a piecewise (multi-`SystemBody`) equation, guarded by `when {D}` — always parameter-only
// in this port (see this fixture's own doc comment in test_util.rs for why a per-index `when`
// guard doesn't work). This exact shape caught a real ScheduledC bug during this suite's own
// development: the guard-selection ternary's ambient context was built with the wrong
// dimensionality, silently rendering an always-false condition (`Y` always took the second
// branch, regardless of `N`) instead of erroring — see `scheduledc.rs::gen_statement_body`'s own
// fix comment.
const PIECEWISE_EQUATION: &str = "affine PiecewiseEquation [N]->{:N>0}
    inputs
        X: [N]
    outputs
        Y: [N]
    when {:N>10} let
        Y[i] = X[i];
    let
        Y[i] = X[i] + X[i];
.";

fn normalized_system(src: &str) -> alpha_transform::ir::System {
    let ctx = isl::Context::new();
    let parse = alpha_syntax::parse(src);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let tree = parse.tree();
    let system = tree.systems().next().expect("one system in fixture");
    let mut resolver = Resolver::new(ctx, &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let (mut ir_system, lower_diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(lower_diagnostics.is_empty(), "{lower_diagnostics:?}");
    alpha_transform::normalize_reduction::apply(&mut ir_system);
    alpha_transform::normalize::apply(ir_system, true)
}

/// Slices out just the loop-iterator declarations + AST-walked statement tree from a whole
/// generated program — the two marker comments `scheduledc.rs::build_driver` emits verbatim
/// bracket exactly that, dropping `#include`s/macros/storage-allocation boilerplate that's
/// incidental to what Tier 3 is actually testing (loop/statement shape), so a preamble-only change
/// elsewhere doesn't churn every snapshot in this file (`docs/codegen-test-design.md` §3).
fn extract_driver_body(code: &str) -> &str {
    let start_marker = "// Evaluate every statement, in schedule order.";
    let end_marker = "// Free all allocated memory.";
    let start = code
        .find(start_marker)
        .unwrap_or_else(|| panic!("marker {start_marker:?} not found in generated code:\n{code}"));
    let end = code
        .find(end_marker)
        .unwrap_or_else(|| panic!("marker {end_marker:?} not found in generated code:\n{code}"));
    assert!(
        start < end,
        "markers out of order in generated code:\n{code}"
    );
    &code[start..end]
}

// -------------------------------------------------------------------------------------------
// §5.2: single-output RHS ladder under an explicit reversing schedule (`i -> N-1-i`).
// -------------------------------------------------------------------------------------------

/// For each representative fixture: confirms the loop direction flips under the reversing
/// schedule while the statement's own assignment text is unchanged — schedule and expression
/// codegen are properly decoupled. `expected_assignment` is checked directly (not just implied by
/// the snapshot) since that's the specific claim this row is making.
fn identity_vs_reversed(src: &str, expected_assignment: &str) -> (String, String) {
    let system = normalized_system(src);
    let identity = alpha_codegen::generate_scheduled_system(&system, "").unwrap();
    let reversed =
        alpha_codegen::generate_scheduled_system(&system, "[N] -> { Y[i] -> [N-1-i]; }").unwrap();
    let identity_body = extract_driver_body(&identity).to_string();
    let reversed_body = extract_driver_body(&reversed).to_string();
    assert!(
        identity_body.contains(expected_assignment),
        "{identity_body}"
    );
    assert!(
        reversed_body.contains(expected_assignment),
        "{reversed_body}"
    );
    (identity_body, reversed_body)
}

#[test]
fn int_literal_identity_schedule() {
    let (identity, _) = identity_vs_reversed(INT_LITERAL, "Y(i) = ((float)(3));");
    insta::assert_snapshot!(identity);
}

#[test]
fn int_literal_reversed_schedule() {
    let (_, reversed) = identity_vs_reversed(INT_LITERAL, "Y(i) = ((float)(3));");
    insta::assert_snapshot!(reversed);
}

#[test]
fn dependence_read_identity_schedule() {
    let (identity, _) = identity_vs_reversed(PLAIN_COPY, "Y(i) = X(i);");
    insta::assert_snapshot!(identity);
}

#[test]
fn dependence_read_reversed_schedule() {
    let (_, reversed) = identity_vs_reversed(PLAIN_COPY, "Y(i) = X(i);");
    insta::assert_snapshot!(reversed);
}

#[test]
fn if_then_else_identity_schedule() {
    let (identity, _) = identity_vs_reversed(
        IF_THEN_ELSE,
        "Y(i) = (((X(i)) > (((float)(0))))) ? (X(i)) : ((-(X(i))));",
    );
    insta::assert_snapshot!(identity);
}

#[test]
fn if_then_else_reversed_schedule() {
    let (_, reversed) = identity_vs_reversed(
        IF_THEN_ELSE,
        "Y(i) = (((X(i)) > (((float)(0))))) ? (X(i)) : ((-(X(i))));",
    );
    insta::assert_snapshot!(reversed);
}

// -------------------------------------------------------------------------------------------
// §5.3: reversing the *inner* reduce dimension for PrefixSum — both legal (the RAW edge is only
// on `i`, §4.2), both numerically correct, loop direction for `j` visibly flips.
// -------------------------------------------------------------------------------------------

#[test]
fn prefix_sum_ascending_inner_reduce_schedule() {
    let system = normalized_system(PREFIX_SUM);
    let text = "{ Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, j]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
    assert!(
        compile_and_run_prefix_sum(&code, "ascending"),
        "ascending-inner-reduce schedule produced incorrect PrefixSum output"
    );
}

#[test]
fn prefix_sum_descending_inner_reduce_schedule() {
    let system = normalized_system(PREFIX_SUM);
    let text = "[N] -> { Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, N-1-j]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
    assert!(
        compile_and_run_prefix_sum(&code, "descending"),
        "descending-inner-reduce schedule produced incorrect PrefixSum output"
    );
}

/// Compiles `generated_c` plus a small driver computing an inclusive prefix sum directly in C,
/// and returns whether they agree — mirrors `scheduledc_e2e.rs`'s own `compile_and_run`, kept as
/// its own small copy here rather than shared, consistent with how the two existing test files
/// already don't share helpers.
fn compile_and_run_prefix_sum(generated_c: &str, tag: &str) -> bool {
    const DRIVER_SRC: &str = r#"
#include <stdio.h>
#include <math.h>
void PrefixSum(long _local_N, float* _local_X, float* _local_Y);
int main() {
    long N = 6;
    float X[6] = {1.0f, 2.0f, 3.0f, 4.0f, 5.0f, 6.0f};
    float Y[6] = {0};
    PrefixSum(N, X, Y);
    float expected[6];
    float running = 0.0f;
    for (int i = 0; i < N; i++) {
        running += X[i];
        expected[i] = running;
    }
    int ok = 1;
    for (int i = 0; i < N; i++) {
        if (fabsf(Y[i] - expected[i]) > 1e-5) {
            fprintf(stderr, "Y[%d] = %f, expected %f\n", i, Y[i], expected[i]);
            ok = 0;
        }
    }
    return ok ? 0 : 1;
}
"#;
    let dir = std::env::temp_dir();
    let c_path = dir.join(format!("alpha_rs_scheduledc_snapshots_{tag}.c"));
    let bin_path = dir.join(format!("alpha_rs_scheduledc_snapshots_{tag}"));
    let mut combined = generated_c.to_string();
    combined.push_str(DRIVER_SRC);
    std::fs::write(&c_path, &combined).expect("writing generated C to a temp file");

    let compile = Command::new("cc")
        .args(["-std=c99", "-o"])
        .arg(&bin_path)
        .arg(&c_path)
        .arg("-lm")
        .output()
        .expect("running cc — a C compiler is required to build this workspace at all");
    assert!(
        compile.status.success(),
        "cc failed to compile generated C ({tag}):\n{}\n--- generated source ---\n{combined}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("running compiled binary ({tag}): {e}"));
    run.status.success()
}

// -------------------------------------------------------------------------------------------
// §5.4 row 6: the flagship example — a `case` expression living inside a reduce's own body.
// This is the one row in the whole matrix that most directly answers "snapshot the loops emitted
// as part of the reduction body": the ternary from `gen_case` living inside the `for j` loop
// nested inside `for i`.
// -------------------------------------------------------------------------------------------

#[test]
fn reduce_case_body_ascending_schedule() {
    let system = normalized_system(REDUCE_CASE_BODY);
    let text = "{ Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, j]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
    assert!(
        compile_and_run_reduce_case_body(&code),
        "reduce-with-case-body schedule produced incorrect numeric output"
    );
}

/// `Y[i] = sum_{j<i} X[j] + 2*X[i]` (the `{:j=i}` branch doubles, `{:j<i}` doesn't) — compiles and
/// runs the generated C, checking against that directly. This numeric check is what caught
/// `REDUCE_CASE_BODY`'s own `auto`-branch bug during this suite's own development (see that
/// fixture's doc comment) — kept here so a regression of the same shape gets caught again, not
/// just a "generates without error" check.
fn compile_and_run_reduce_case_body(generated_c: &str) -> bool {
    const DRIVER_SRC: &str = r#"
#include <stdio.h>
#include <math.h>
void ReduceCaseBody(long _local_N, float* _local_X, float* _local_Y);
int main() {
    long N = 6;
    float X[6] = {1.0f, 2.0f, 3.0f, 4.0f, 5.0f, 6.0f};
    float Y[6] = {0};
    ReduceCaseBody(N, X, Y);
    float expected[6];
    float running = 0.0f;
    for (int i = 0; i < N; i++) {
        expected[i] = running + 2.0f * X[i];
        running += X[i];
    }
    int ok = 1;
    for (int i = 0; i < N; i++) {
        if (fabsf(Y[i] - expected[i]) > 1e-5) {
            fprintf(stderr, "Y[%d] = %f, expected %f\n", i, Y[i], expected[i]);
            ok = 0;
        }
    }
    return ok ? 0 : 1;
}
"#;
    let dir = std::env::temp_dir();
    let c_path = dir.join("alpha_rs_scheduledc_snapshots_reduce_case_body.c");
    let bin_path = dir.join("alpha_rs_scheduledc_snapshots_reduce_case_body");
    let mut combined = generated_c.to_string();
    combined.push_str(DRIVER_SRC);
    std::fs::write(&c_path, &combined).expect("writing generated C to a temp file");
    let compile = Command::new("cc")
        .args(["-std=c99", "-o"])
        .arg(&bin_path)
        .arg(&c_path)
        .arg("-lm")
        .output()
        .expect("running cc — a C compiler is required to build this workspace at all");
    assert!(
        compile.status.success(),
        "cc failed to compile generated C:\n{}\n--- generated source ---\n{combined}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("running compiled binary: {e}"));
    run.status.success()
}

// -------------------------------------------------------------------------------------------
// §5.5 row 1: two independent outputs (no shared data) — fused (same loop level) vs. separate
// (sequential loops). This is the concrete "fusing" example from the design doc's own prompt.
// -------------------------------------------------------------------------------------------

#[test]
fn two_independent_outputs_default_identity_is_legal() {
    let system = normalized_system(TWO_INDEPENDENT);
    let code = alpha_codegen::generate_scheduled_system(&system, "").unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

#[test]
fn two_independent_outputs_fused_same_loop_level() {
    let system = normalized_system(TWO_INDEPENDENT);
    let text = "{ Y[i] -> [i, 0]; W[i] -> [i, 1]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

#[test]
fn two_independent_outputs_separate_sequential_loops() {
    let system = normalized_system(TWO_INDEPENDENT);
    let text = "{ Y[i] -> [0, i]; W[i] -> [1, i]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

// -------------------------------------------------------------------------------------------
// §5.5 row 2: producer -> consumer, no reduce. Legal sequential vs. illegal reversed (the
// consumer reading before the producer has written).
// -------------------------------------------------------------------------------------------

#[test]
fn producer_consumer_no_reduce_legal_sequential_schedule() {
    let system = normalized_system(PRODUCER_CONSUMER_NO_REDUCE);
    let text = "{ Y[i] -> [0, i]; W[i] -> [1, i]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

#[test]
fn producer_consumer_no_reduce_reversed_schedule_is_illegal() {
    let system = normalized_system(PRODUCER_CONSUMER_NO_REDUCE);
    // W (the consumer) now runs *before* Y (the producer) it reads.
    let text = "{ Y[i] -> [1, i]; W[i] -> [0, i]; }";
    let err = match alpha_codegen::generate_scheduled_system(&system, text) {
        Ok(_) => panic!("expected reading before the producer to be illegal"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("W") && err.contains("§7.2"), "{err}");
    insta::assert_snapshot!(err);
}

// -------------------------------------------------------------------------------------------
// §5.5 row 3: producer -> consumer through a reduce (PrefixSumConsumer). A fully-legal explicit
// schedule threading the reduce pair and its ordinary consumer together.
// -------------------------------------------------------------------------------------------

#[test]
fn prefix_sum_with_consumer_legal_schedule() {
    let system = normalized_system(PREFIX_SUM_WITH_CONSUMER);
    let text = "{ Y__init[i] -> [i, 0, 0]; \
                 Y__reduce[i,j] -> [i, 1, j]; \
                 Z[i] -> [i, 2, 0]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

// -------------------------------------------------------------------------------------------
// §5.5 row 4: two independent reduces fan out from one input (statement enumeration already
// covered in describe.rs's own Tier-4 test) — here, the same fused-vs-separate contrast as row 1,
// this time across two full reduce pairs (4 statements) instead of two ordinary statements.
// -------------------------------------------------------------------------------------------

#[test]
fn two_reduces_fan_out_fused_schedule() {
    let system = normalized_system(TWO_REDUCES_FAN_OUT);
    let text = "{ Y__init[i] -> [i, 0, 0]; MaxY__init[i] -> [i, 0, 0]; \
                 Y__reduce[i,j] -> [i, 1, j]; MaxY__reduce[i,j] -> [i, 1, j]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

#[test]
fn two_reduces_fan_out_separate_schedule() {
    let system = normalized_system(TWO_REDUCES_FAN_OUT);
    let text = "{ Y__init[i] -> [0, i, 0, 0]; Y__reduce[i,j] -> [0, i, 1, j]; \
                 MaxY__init[i] -> [1, i, 0, 0]; MaxY__reduce[i,j] -> [1, i, 1, j]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

// -------------------------------------------------------------------------------------------
// §5.5 row 5: nested reduce dependency (prefix-sum-of-prefix-sum) — the richest legality case.
// `Y`'s *entire* accumulation for a given `i` must complete before any `Z__reduce` instance reads
// `Y[j]` for `j<=i`; a per-`i` interleaving that lets `Z__reduce` read `Y[i]` before that same
// iteration's own `Y__reduce(i,i)` has run is illegal, even though `Y[0..i-1]` is already safe.
// -------------------------------------------------------------------------------------------

#[test]
fn nested_reduce_dependency_legal_fully_sequential_schedule() {
    let system = normalized_system(NESTED_REDUCE_DEPENDENCY);
    // All of Y (every i) completes before any of Z starts.
    let text = "{ Y__init[i] -> [0, i, 0, 0]; Y__reduce[i,j] -> [0, i, 1, j]; \
                 Z__init[i] -> [1, i, 0, 0]; Z__reduce[i,j] -> [1, i, 1, j]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
}

#[test]
fn nested_reduce_dependency_illegal_interleaved_schedule() {
    let system = normalized_system(NESTED_REDUCE_DEPENDENCY);
    // For each i: init both, then Z__reduce(i,*) *before* Y__reduce(i,*) — by the time
    // Z__reduce(i,i) reads Y[i], this same iteration's own Y__reduce(i,i) hasn't added X[i] into
    // Y[i] yet (it's phase 3, later than Z__reduce's phase 1).
    let text = "{ Y__init[i] -> [i, 0, 0]; Z__init[i] -> [i, 0, 1]; \
                 Z__reduce[i,j] -> [i, 1, j]; Y__reduce[i,j] -> [i, 2, j]; }";
    let err = match alpha_codegen::generate_scheduled_system(&system, text) {
        Ok(_) => panic!("expected this interleaving to be illegal"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("§7.2"), "{err}");
    insta::assert_snapshot!(err);
}

// -------------------------------------------------------------------------------------------
// §5.6: piecewise equation — the guard-selection ternary chain, with a numeric check on *both*
// sides of the guard (not just "generates without error"), since that's exactly the level at
// which this fixture's own bug (see its doc comment above) was only actually caught.
// -------------------------------------------------------------------------------------------

#[test]
fn piecewise_equation_identity_schedule() {
    let system = normalized_system(PIECEWISE_EQUATION);
    let code = alpha_codegen::generate_scheduled_system(&system, "").unwrap();
    insta::assert_snapshot!(extract_driver_body(&code).to_string());
    assert!(
        compile_and_run_piecewise(&code, 20, true),
        "N=20 (>10): should take the guarded X[i] branch"
    );
    assert!(
        compile_and_run_piecewise(&code, 5, false),
        "N=5 (<=10): should take the default X[i]+X[i] branch"
    );
}

/// Compiles `generated_c` plus a driver that runs `PiecewiseEquation` at a chosen `n`, checking
/// `Y[i]` against whichever branch `expect_guarded_branch` says should fire — `Y[i] = X[i]` if
/// `N>10`, `Y[i] = 2*X[i]` otherwise.
fn compile_and_run_piecewise(generated_c: &str, n: i64, expect_guarded_branch: bool) -> bool {
    let driver = format!(
        r#"
#include <stdio.h>
#include <math.h>
void PiecewiseEquation(long _local_N, float* _local_X, float* _local_Y);
int main() {{
    long N = {n};
    float X[{n}];
    float Y[{n}];
    for (int i = 0; i < N; i++) {{
        X[i] = (float)(i + 1);
        Y[i] = 0.0f;
    }}
    PiecewiseEquation(N, X, Y);
    int ok = 1;
    for (int i = 0; i < N; i++) {{
        float expected = {mult}f * X[i];
        if (fabsf(Y[i] - expected) > 1e-5) {{
            fprintf(stderr, "Y[%d] = %f, expected %f\n", i, Y[i], expected);
            ok = 0;
        }}
    }}
    return ok ? 0 : 1;
}}
"#,
        n = n,
        mult = if expect_guarded_branch { "1.0" } else { "2.0" }
    );
    let dir = std::env::temp_dir();
    let tag = format!("piecewise_{n}");
    let c_path = dir.join(format!("alpha_rs_scheduledc_snapshots_{tag}.c"));
    let bin_path = dir.join(format!("alpha_rs_scheduledc_snapshots_{tag}"));
    let mut combined = generated_c.to_string();
    combined.push_str(&driver);
    std::fs::write(&c_path, &combined).expect("writing generated C to a temp file");
    let compile = Command::new("cc")
        .args(["-std=c99", "-o"])
        .arg(&bin_path)
        .arg(&c_path)
        .arg("-lm")
        .output()
        .expect("running cc — a C compiler is required to build this workspace at all");
    assert!(
        compile.status.success(),
        "cc failed to compile generated C ({tag}):\n{}\n--- generated source ---\n{combined}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("running compiled binary ({tag}): {e}"));
    run.status.success()
}

// -------------------------------------------------------------------------------------------
// §5.9: a couple of full-file snapshots, as a coarser sanity net catching preamble/storage/macro
// changes the extracted-body slices above deliberately don't cover. Low volume on purpose — these
// are the snapshots most prone to unrelated-change churn.
// -------------------------------------------------------------------------------------------

#[test]
fn prefix_sum_full_generated_file() {
    let system = normalized_system(PREFIX_SUM);
    let text = "{ Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, j]; }";
    let code = alpha_codegen::generate_scheduled_system(&system, text).unwrap();
    insta::assert_snapshot!(code);
}
