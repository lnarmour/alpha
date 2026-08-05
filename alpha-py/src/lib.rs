//! PyO3 binding exposing `read`/`normalize`/`schedule`/`generate` to a Jupyter notebook — §10.1 of
//! `docs/scheduled-codegen-design.md`. Three immutable, distinctly-typed Python classes
//! (`System`/`NormalizedSystem`/`ScheduledSystem`) wrap a cloned `alpha_transform::ir::System`;
//! every function/method here clones its receiver's underlying Rust value, runs the existing
//! (in-place-mutating) Rust pass over the clone, and wraps the result — never touching the
//! original (§5.1, §5.2). `System`/`NormalizedSystem`/`ScheduledSystem` being genuinely different
//! Python types is what turns §5.1's normalized-IR precondition into a `TypeError` the binding
//! raises before any Rust code runs, rather than a runtime diagnostic deep inside the pipeline.

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use std::path::Path;

/// A parsed-and-lowered system, pre-normalization. `%%alpha`'s own return type (§5.2) —
/// `System.__repr__` shows one entry per output/local variable at its identity schedule, no
/// reduce split (that split doesn't exist until `normalize_reduction` runs, §4.2).
///
/// `unsendable`: isl's own `Context`/`Set`/`Map`/... (transitively inside every `ir::System`) wrap
/// raw C pointers with non-atomic, non-thread-safe refcounting — isl itself is not thread-safe.
/// PyO3 defaults to requiring `Sync` for a `#[pyclass]` (Python objects can in principle be
/// touched from multiple threads); `unsendable` is the correct, honest opt-out, restricting a
/// value to the thread that created it — exactly isl's own real constraint, not a workaround.
#[pyclass(module = "alpha", frozen, unsendable)]
struct System(alpha_transform::ir::System);

/// The output of `alpha.normalize()` — both normalizing passes have run, in the order this port
/// requires (§1, §3): `normalize_reduction::apply` then `normalize::apply`. Only a
/// `NormalizedSystem` can be scheduled or generated (§5.1) — passing a bare `System` to either is
/// a `TypeError`, not a pipeline diagnostic.
#[pyclass(module = "alpha", frozen, unsendable)]
struct NormalizedSystem(alpha_transform::ir::System);

/// The output of `NormalizedSystem.schedule(text)` — a validated (§6), legality-checked (§7)
/// target mapping has been applied. `ScheduledSystem.__repr__` reflects the schedule that was
/// actually loaded, not the identity default.
#[pyclass(module = "alpha", frozen, unsendable)]
struct ScheduledSystem {
    system: alpha_transform::ir::System,
    schedule_text: String,
}

// A target mapping (§6) failed to parse or validate, or passed validation but reorders a real
// dependence (§7) — both map to this one Python exception for now (§13: exact error-type shape,
// finer-grained than this, is an implementation-time decision deferred past v1).
//
// `create_exception!` (rather than a hand-rolled `#[pyclass(extends = PyException)]`) is what
// gives the type a working `__new__` — a manually authored unit-struct pyclass has no
// constructor PyO3 can call when raising it, which surfaces at runtime as `TypeError: No
// constructor defined for ScheduleError` the moment Python code actually catches one.
pyo3::create_exception!(_alpha, ScheduleError, pyo3::exceptions::PyException);

fn codegen_err_to_py(e: alpha_codegen::CodegenError) -> PyErr {
    use alpha_codegen::CodegenError as E;
    match e {
        E::InvalidSchedule(msg) | E::IllegalSchedule(msg) => PyErr::new::<ScheduleError, _>(msg),
        other => PyValueError::new_err(other.to_string()),
    }
}

/// Parse + `analyze_system` + `lower_system` — the three steps `%%alpha` (§5.2) and
/// [`read`] both go through, on inline source text either way; the only difference is where the
/// text comes from.
fn parse_and_lower(source: &str) -> PyResult<alpha_transform::ir::System> {
    let parse = alpha_syntax::parse(source);
    if !parse.errors.is_empty() {
        let msgs: Vec<String> = parse
            .errors
            .iter()
            .map(|e| format!("{}..{}: {}", e.start, e.end, e.message))
            .collect();
        return Err(PyValueError::new_err(format!(
            "{} syntax error(s):\n{}",
            parse.errors.len(),
            msgs.join("\n")
        )));
    }
    let tree = parse.tree();
    let Some(system) = tree.systems().next() else {
        return Err(PyValueError::new_err("no systems found in source"));
    };
    let ctx = isl::Context::new();
    let mut resolver = alpha_model::Resolver::new(ctx, &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    if !diagnostics.is_empty() {
        let msgs: Vec<String> = diagnostics.iter().map(|d| d.to_string()).collect();
        return Err(PyValueError::new_err(format!(
            "{} diagnostic(s):\n{}",
            diagnostics.len(),
            msgs.join("\n")
        )));
    }
    let (ir_system, lower_diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system)
            .map_err(|e| PyValueError::new_err(format!("internal error lowering system: {e}")))?;
    if !lower_diagnostics.is_empty() {
        let msgs: Vec<String> = lower_diagnostics.iter().map(|d| d.to_string()).collect();
        return Err(PyValueError::new_err(format!(
            "{} equation(s) skipped during lowering:\n{}",
            lower_diagnostics.len(),
            msgs.join("\n")
        )));
    }
    Ok(ir_system)
}

#[pymethods]
impl System {
    fn __repr__(&self) -> PyResult<String> {
        alpha_codegen::describe_system(&self.0).map_err(codegen_err_to_py)
    }
}

#[pymethods]
impl NormalizedSystem {
    fn __repr__(&self) -> PyResult<String> {
        alpha_codegen::describe_normalized_system(&self.0, "").map_err(codegen_err_to_py)
    }

    /// Parses, validates (§6), and legality-checks (§7) `text` against `self` — a clone of
    /// `self`'s own underlying system, never `self` itself, so a rejected schedule leaves nothing
    /// to roll back (there's no mutation to undo in the first place). Validation only — no codegen
    /// runs here; `generate()` (§10.1) is a separate, possibly-never-called step.
    fn schedule(&self, text: &str) -> PyResult<ScheduledSystem> {
        let system = self.0.clone();
        alpha_codegen::validate_scheduled_system(&system, text).map_err(codegen_err_to_py)?;
        Ok(ScheduledSystem {
            system,
            schedule_text: text.to_string(),
        })
    }
}

#[pymethods]
impl ScheduledSystem {
    fn __repr__(&self) -> PyResult<String> {
        alpha_codegen::describe_normalized_system(&self.system, &self.schedule_text)
            .map_err(codegen_err_to_py)
    }
}

/// Loads a `.alpha` file from disk — same three steps as `%%alpha` (§5.2), for use outside a
/// notebook (e.g. a plain script).
#[pyfunction]
fn read(path: &str) -> PyResult<System> {
    let source = std::fs::read_to_string(Path::new(path))
        .map_err(|e| PyValueError::new_err(format!("reading {path:?}: {e}")))?;
    Ok(System(parse_and_lower(&source)?))
}

/// Parses `source` directly — what `%%alpha`'s own cell magic (§5.2, §10.2) is sugar for.
#[pyfunction]
fn parse(source: &str) -> PyResult<System> {
    Ok(System(parse_and_lower(source)?))
}

/// Normalizes a *clone* of `sys`'s underlying IR — `normalize_reduction::apply` then
/// `normalize::apply` (§1, §3 — this order is required for this port, see `alpha-transform`'s own
/// module doc), bundled into one Python call so nothing downstream has to sequence them itself.
/// `sys` itself is untouched and still usable.
#[pyfunction]
fn normalize(sys: &System) -> NormalizedSystem {
    let mut system = sys.0.clone();
    alpha_transform::normalize_reduction::apply(&mut system);
    let normalized = alpha_transform::normalize::apply(system, true);
    NormalizedSystem(normalized)
}

/// Generates C source via `ScheduledC` (§8). Accepts either a `NormalizedSystem` (sugar for
/// scheduling it with empty text first — §6's identity-schedule default) or a `ScheduledSystem`.
#[pyfunction]
fn generate(system: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(scheduled) = system.extract::<PyRef<ScheduledSystem>>() {
        return alpha_codegen::generate_scheduled_system(
            &scheduled.system,
            &scheduled.schedule_text,
        )
        .map_err(codegen_err_to_py);
    }
    if let Ok(normalized) = system.extract::<PyRef<NormalizedSystem>>() {
        return alpha_codegen::generate_scheduled_system(&normalized.0, "")
            .map_err(codegen_err_to_py);
    }
    Err(PyTypeError::new_err(
        "generate() requires a NormalizedSystem or ScheduledSystem, not a bare System — run \
         alpha.normalize() first (§5.1)",
    ))
}

#[pymodule]
fn _alpha(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<System>()?;
    m.add_class::<NormalizedSystem>()?;
    m.add_class::<ScheduledSystem>()?;
    m.add("ScheduleError", m.py().get_type::<ScheduleError>())?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(normalize, m)?)?;
    m.add_function(wrap_pyfunction!(generate, m)?)?;
    Ok(())
}
