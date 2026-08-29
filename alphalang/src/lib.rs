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
use pyo3::types::PyTuple;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

#[pyclass(module = "alpha", frozen)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Multiplicity(alpha_model::Multiplicity);

#[pymethods]
impl Multiplicity {
    #[classattr]
    #[pyo3(name = "LINEAR")]
    fn linear() -> Self {
        Self(alpha_model::Multiplicity::Linear)
    }

    #[classattr]
    #[pyo3(name = "UNRESTRICTED")]
    fn unrestricted() -> Self {
        Self(alpha_model::Multiplicity::Unrestricted)
    }

    fn __repr__(&self) -> &'static str {
        match self.0 {
            alpha_model::Multiplicity::Linear => "Multiplicity.LINEAR",
            alpha_model::Multiplicity::Unrestricted => "Multiplicity.UNRESTRICTED",
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self == other
    }
}

#[pyclass(module = "alpha", frozen)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct ElementType(alpha_model::ElementType);

#[pymethods]
impl ElementType {
    #[classattr]
    #[pyo3(name = "UNSPECIFIED")]
    fn unspecified() -> Self {
        Self(alpha_model::ElementType::Unspecified)
    }

    #[classattr]
    #[pyo3(name = "BOOL")]
    fn bool() -> Self {
        Self(alpha_model::ElementType::Bool)
    }

    #[classattr]
    #[pyo3(name = "INT")]
    fn int() -> Self {
        Self(alpha_model::ElementType::Int)
    }

    #[classattr]
    #[pyo3(name = "REAL")]
    fn real() -> Self {
        Self(alpha_model::ElementType::Real)
    }

    #[classattr]
    #[pyo3(name = "QUBIT")]
    fn qubit() -> Self {
        Self(alpha_model::ElementType::Qubit)
    }

    fn __repr__(&self) -> &'static str {
        match self.0 {
            alpha_model::ElementType::Unspecified => "ElementType.UNSPECIFIED",
            alpha_model::ElementType::Bool => "ElementType.BOOL",
            alpha_model::ElementType::Int => "ElementType.INT",
            alpha_model::ElementType::Real => "ElementType.REAL",
            alpha_model::ElementType::Qubit => "ElementType.QUBIT",
        }
    }

    fn __eq__(&self, other: &Self) -> bool {
        self == other
    }
}

#[pyclass(module = "alpha", frozen)]
struct Variable {
    name: String,
    domain: String,
    multiplicity: Multiplicity,
    element_type: ElementType,
}

impl From<&alpha_transform::ir::Variable> for Variable {
    fn from(variable: &alpha_transform::ir::Variable) -> Self {
        Self {
            name: variable.name.clone(),
            domain: variable.domain.to_string(),
            multiplicity: Multiplicity(variable.multiplicity),
            element_type: ElementType(variable.element_type),
        }
    }
}

#[pymethods]
impl Variable {
    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    #[getter]
    fn domain(&self) -> &str {
        &self.domain
    }

    #[getter]
    fn multiplicity(&self) -> Multiplicity {
        self.multiplicity
    }

    #[getter]
    fn element_type<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let name = match self.element_type.0 {
            alpha_model::ElementType::Unspecified => "UNSPECIFIED",
            alpha_model::ElementType::Bool => "BOOL",
            alpha_model::ElementType::Int => "INT",
            alpha_model::ElementType::Real => "REAL",
            alpha_model::ElementType::Qubit => "QUBIT",
        };
        py.get_type::<ElementType>().getattr(name)
    }

    fn __repr__(&self) -> String {
        format!(
            "Variable(name='{}', domain='{}', multiplicity={}, element_type={})",
            self.name,
            self.domain,
            self.multiplicity.__repr__(),
            self.element_type.__repr__()
        )
    }
}

fn variable_tuple<'py>(
    py: Python<'py>,
    variables: &[alpha_transform::ir::Variable],
) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(py, variables.iter().map(Variable::from))
}

/// A parsed-and-lowered system, pre-normalization. `%%alphalang`'s own return type (§5.2) —
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

/// Parse + `analyze_system` + `lower_system` — the three steps `%%alphalang` (§5.2) and
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
    let analyses = alpha_model::analyze_root(&ctx, &tree);
    let diagnostics = analyses
        .first()
        .map(|(_, diagnostics)| diagnostics.as_slice())
        .unwrap_or_default();
    if !diagnostics.is_empty() {
        let msgs: Vec<String> = diagnostics.iter().map(|d| d.to_string()).collect();
        return Err(PyValueError::new_err(format!(
            "{} diagnostic(s):\n{}",
            diagnostics.len(),
            msgs.join("\n")
        )));
    }
    let mut resolver = alpha_model::Resolver::new(ctx, &system);
    let (_, _, resolver_diagnostics) = resolver.analyze_system(&system);
    if !resolver_diagnostics.is_empty() {
        let msgs: Vec<String> = resolver_diagnostics.iter().map(|d| d.to_string()).collect();
        return Err(PyValueError::new_err(format!(
            "{} diagnostic(s):\n{}",
            resolver_diagnostics.len(),
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
    #[getter]
    fn inputs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.0.inputs)
    }

    #[getter]
    fn outputs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.0.outputs)
    }

    #[getter]
    fn locals<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.0.locals)
    }

    fn __repr__(&self) -> PyResult<String> {
        alpha_codegen::describe_system(&self.0).map_err(codegen_err_to_py)
    }
}

#[pymethods]
impl NormalizedSystem {
    #[getter]
    fn inputs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.0.inputs)
    }

    #[getter]
    fn outputs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.0.outputs)
    }

    #[getter]
    fn locals<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.0.locals)
    }

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
    #[getter]
    fn inputs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.system.inputs)
    }

    #[getter]
    fn outputs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.system.outputs)
    }

    #[getter]
    fn locals<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        variable_tuple(py, &self.system.locals)
    }

    fn __repr__(&self) -> PyResult<String> {
        alpha_codegen::describe_normalized_system(&self.system, &self.schedule_text)
            .map_err(codegen_err_to_py)
    }
}

/// Loads a `.alpha` file from disk — same three steps as `%%alphalang` (§5.2), for use outside a
/// notebook (e.g. a plain script).
#[pyfunction]
fn read(path: &str) -> PyResult<System> {
    let source = std::fs::read_to_string(Path::new(path))
        .map_err(|e| PyValueError::new_err(format!("reading {path:?}: {e}")))?;
    Ok(System(parse_and_lower(&source)?))
}

/// Parses `source` directly — what `%%alphalang`'s own cell magic (§5.2, §10.2) is sugar for.
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

/// Generates a concrete scheduled HUGR envelope. Parameter values specialize all symbolic
/// domains before resource realization, so this API never emits a parametric HUGR.
#[pyfunction]
fn generate_hugr(system: &Bound<'_, PyAny>, parameters: HashMap<String, i64>) -> PyResult<String> {
    let parameters = parameters.into_iter().collect::<BTreeMap<_, _>>();
    if let Ok(scheduled) = system.extract::<PyRef<ScheduledSystem>>() {
        validate_parameter_names(&scheduled.system, &parameters)?;
        return alpha_codegen::generate_hugr_system(
            &scheduled.system,
            &scheduled.schedule_text,
            &parameters,
        )
        .map_err(codegen_err_to_py);
    }
    if let Ok(normalized) = system.extract::<PyRef<NormalizedSystem>>() {
        validate_parameter_names(&normalized.0, &parameters)?;
        return alpha_codegen::generate_hugr_system(&normalized.0, "", &parameters)
            .map_err(codegen_err_to_py);
    }
    Err(PyTypeError::new_err(
        "generate_hugr() requires a NormalizedSystem or ScheduledSystem, not a bare System — run alpha.normalize() first",
    ))
}

fn validate_parameter_names(
    system: &alpha_transform::ir::System,
    parameters: &BTreeMap<String, i64>,
) -> PyResult<()> {
    let space = system.parameter_domain.space();
    let expected = (0..space.dim(isl::DimType::Param))
        .map(|position| {
            space
                .dim_name(isl::DimType::Param, position)
                .ok_or_else(|| PyValueError::new_err("system parameter has no name"))
        })
        .collect::<PyResult<std::collections::BTreeSet<_>>>()?;
    for name in &expected {
        if !parameters.contains_key(name) {
            return Err(PyValueError::new_err(format!(
                "specialization failed: missing parameter '{name}'"
            )));
        }
    }
    for name in parameters.keys() {
        if !expected.contains(name) {
            return Err(PyValueError::new_err(format!(
                "specialization failed: unknown parameter '{name}'"
            )));
        }
    }
    Ok(())
}

/// Any of the three pipeline-stage wrapper types share the same underlying `ir::System` — the
/// three printers below (`print`/`show`/`ashow`) work identically at any stage, unlike
/// `schedule`/`generate`, which are gated on normalization (§5.1). Clones the same as every other
/// binding here (§5.2's "every stage is a pure function" contract) even though a printer never
/// mutates its input — consistent with the rest of this module rather than a special case.
fn extract_ir_system(system: &Bound<'_, PyAny>) -> PyResult<alpha_transform::ir::System> {
    if let Ok(s) = system.extract::<PyRef<System>>() {
        return Ok(s.0.clone());
    }
    if let Ok(s) = system.extract::<PyRef<NormalizedSystem>>() {
        return Ok(s.0.clone());
    }
    if let Ok(s) = system.extract::<PyRef<ScheduledSystem>>() {
        return Ok(s.system.clone());
    }
    Err(PyTypeError::new_err(
        "expected a System, NormalizedSystem, or ScheduledSystem",
    ))
}

/// An indented debug tree dump — every node's own kind plus its `expression_domain`/
/// `context_domain` — ported from alpha-language's `PrintAST.xtend`. For inspecting exactly what
/// a pass did to the tree; see [`show`]/[`ashow`] to read a program back as source.
#[pyfunction]
fn print(system: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(alpha_transform::print::print_ast(&extract_ir_system(
        system,
    )?))
}

/// Reconstructs Alpha-like source syntax from `system` — ported from alpha-language's
/// `Show.xtend`. A `Dependence` prints in point-free composition form (`f@X`); see [`ashow`] for
/// array-index notation (`X[f]`) instead.
#[pyfunction]
fn show(system: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(alpha_transform::print::show(&extract_ir_system(system)?))
}

/// Like [`show`], but renders a `Dependence` over a `Variable`/constant in array-index notation
/// (`X[f]`) and shows each equation's own ambient index names explicitly (`X[i,j] = ...`) —
/// ported from alpha-language's `AShow.xtend`.
#[pyfunction]
fn ashow(system: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(alpha_transform::print::ashow(&extract_ir_system(system)?))
}

#[pymodule]
fn _alpha(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Multiplicity>()?;
    m.add_class::<ElementType>()?;
    m.add_class::<Variable>()?;
    m.add_class::<System>()?;
    m.add_class::<NormalizedSystem>()?;
    m.add_class::<ScheduledSystem>()?;
    m.add("ScheduleError", m.py().get_type::<ScheduleError>())?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(parse, m)?)?;
    m.add_function(wrap_pyfunction!(normalize, m)?)?;
    m.add_function(wrap_pyfunction!(generate, m)?)?;
    m.add_function(wrap_pyfunction!(generate_hugr, m)?)?;
    m.add_function(wrap_pyfunction!(print, m)?)?;
    m.add_function(wrap_pyfunction!(show, m)?)?;
    m.add_function(wrap_pyfunction!(ashow, m)?)?;
    Ok(())
}
