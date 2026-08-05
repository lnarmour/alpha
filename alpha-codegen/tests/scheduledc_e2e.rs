//! End-to-end verification of `ScheduledC` (§8 of `docs/scheduled-codegen-design.md`): generates
//! real C for `PrefixScan.alpha` (a fixture with a genuine reduce dependency), compiles it with
//! the system's own `cc`, links it against a small driver, runs the resulting binary, and checks
//! the actual numeric output — not just that generation succeeds. This is the level at which
//! several real bugs were only actually caught during implementation (an unindexed reduce
//! accumulator cell, undeclared loop iterators, sibling statements redeclaring the same local in
//! one shared scope) — generation succeeding and even "looking right" on inspection wasn't enough
//! to catch any of them. Also cross-checks against `WriteC`'s own independently-generated code for
//! the same program, on the same input, as a second, independent confirmation.
//!
//! Requires a C compiler (`cc`) on `PATH` — already a hard requirement to build this workspace at
//! all (`isl-sys`/`barvinok-sys`'s own `build.rs` compile C/C++), so not an extra dependency this
//! test introduces.

use alpha_model::Resolver;
use std::path::PathBuf;
use std::process::Command;

const PREFIX_SCAN_SRC: &str = "affine PrefixScan [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
.";

/// `Y[i] = reduce(+, [j], {:j<=i}: X[j])` is an *inclusive* prefix sum (`j<=i`, not `j<i`) — a
/// driver `main()` that calls the generated `PrefixScan`, computes the same sum directly in C,
/// and exits non-zero if they disagree by more than a float epsilon.
const DRIVER_SRC: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

void PrefixScan(long _local_N, float* _local_X, float* _local_Y);

int main() {
    long N = 6;
    float X[6] = {1.0f, 2.0f, 3.0f, 4.0f, 5.0f, 6.0f};
    float Y[6] = {0};
    PrefixScan(N, X, Y);

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

fn lowered_prefix_scan() -> alpha_transform::ir::System {
    let ctx = isl::Context::new();
    let parse = alpha_syntax::parse(PREFIX_SCAN_SRC);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let tree = parse.tree();
    let system = tree.systems().next().expect("one system in fixture");
    let mut resolver = Resolver::new(ctx, &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let (ir_system, lower_diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(lower_diagnostics.is_empty(), "{lower_diagnostics:?}");
    ir_system
}

/// Compiles `generated_c` plus the shared driver into a binary and runs it, returning whether the
/// numeric check in `DRIVER_SRC` passed. Panics (rather than returning a `Result`) on any
/// tooling failure (missing `cc`, compile error) — those indicate a real generation bug, not an
/// expected/ignorable condition for this test.
fn compile_and_run(generated_c: &str, tag: &str) -> bool {
    let dir = std::env::temp_dir();
    let c_path: PathBuf = dir.join(format!("alpha_rs_scheduledc_e2e_{tag}.c"));
    let bin_path: PathBuf = dir.join(format!("alpha_rs_scheduledc_e2e_{tag}"));
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

#[test]
fn scheduled_c_prefix_scan_matches_expected_output() {
    let mut ir_system = lowered_prefix_scan();
    alpha_transform::normalize_reduction::apply(&mut ir_system);
    let normalized = alpha_transform::normalize::apply(ir_system, true);

    let text = "{ Y__init[i] -> [i, 0, 0]; \
                 Y__reduce[i,j] -> [i, 1, j]; }";
    let generated = alpha_codegen::generate_scheduled_system(&normalized, text)
        .expect("ScheduledC generation should succeed for a properly-padded explicit schedule");

    assert!(
        compile_and_run(&generated, "scheduledc"),
        "ScheduledC-generated PrefixScan produced incorrect output"
    );
}

#[test]
fn scheduled_c_agrees_with_write_c_on_the_same_program() {
    // Independent cross-check: two separately-implemented backends, run against the same input,
    // should agree — a much stronger signal than either one individually "looking right".
    // `ir::System` has no `Clone` (isl objects aren't trivially copyable across two independent
    // owning trees the way this needs), so each backend gets its own fresh lowering from source —
    // simple and cheap enough for a two-statement test fixture.
    let mut for_scheduled = lowered_prefix_scan();
    alpha_transform::normalize_reduction::apply(&mut for_scheduled);
    let normalized_scheduled = alpha_transform::normalize::apply(for_scheduled, true);
    let text = "{ Y__init[i] -> [i, 0, 0]; \
                 Y__reduce[i,j] -> [i, 1, j]; }";
    let scheduled_c = alpha_codegen::generate_scheduled_system(&normalized_scheduled, text)
        .expect("ScheduledC generation should succeed");

    // WriteC's own required pass order (§3 of the design) — reduction-hoisting first — happens to
    // be the same order used above, but that's incidental; each backend's own precondition is
    // documented and enforced independently (§1, §5.1).
    let mut for_write_c = lowered_prefix_scan();
    alpha_transform::normalize_reduction::apply(&mut for_write_c);
    let normalized_write_c = alpha_transform::normalize::apply(for_write_c, true);
    let write_c = alpha_codegen::generate_system(&normalized_write_c)
        .expect("WriteC generation should succeed");

    assert!(
        compile_and_run(&scheduled_c, "cross_scheduledc"),
        "ScheduledC's output failed the numeric check"
    );
    assert!(
        compile_and_run(&write_c, "cross_writec"),
        "WriteC's output failed the numeric check"
    );
}
