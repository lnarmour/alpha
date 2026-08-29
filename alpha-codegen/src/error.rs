//! `alpha-codegen`'s own error type — distinct from `alpha_model::Diagnostic` (that enum is
//! deliberately closed over the source system's own semantic-analysis catalog; codegen-time
//! failures are a different concern: an isl operation failing on an already-analyzed tree
//! shouldn't happen but is still reported, not panicked on, and a handful of documented,
//! deliberate scope boundaries the source system's own `WriteC` shares or that this port defers.

use std::fmt;

#[derive(Debug, Clone)]
pub enum CodegenError {
    Isl(isl::IslError),
    /// A construct this port's `WriteC` doesn't generate code for. Carries a short description of
    /// what and, where useful, why — e.g. `UseEquation` (no codegen backend in the source system
    /// either), `Select`/`IndexPolynomial` (relation-based
    /// reindexing / piecewise-polynomial index values — real but rare in the fixture corpus, not
    /// implemented this session), `ArgReduce` (unseen across the whole 82-fixture corpus).
    Unsupported(String),
    /// A target mapping (`docs/scheduled-codegen-design.md` §6) failed to parse, or failed one of
    /// §6's validation rules (unknown tuple name, non-total/non-injective schedule, mismatched
    /// shared-schedule-space width) — distinct from [`Self::Isl`] (an isl operation failing on
    /// already-validated input, which shouldn't happen) and [`Self::Unsupported`] (a construct
    /// with no codegen backend at all). Carries a diagnostic naming the offending statement/rule.
    InvalidSchedule(String),
    /// A target mapping parsed and validated (§6) but reorders a real dependence — §7's legality
    /// check. Carries a diagnostic naming the two statements and the dependence between them.
    IllegalSchedule(String),
    Specialization(String),
    Realization(String),
    Hugr(String),
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::Isl(e) => write!(f, "isl error: {e}"),
            CodegenError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
            CodegenError::InvalidSchedule(msg) => write!(f, "invalid target mapping: {msg}"),
            CodegenError::IllegalSchedule(msg) => write!(f, "illegal schedule: {msg}"),
            CodegenError::Specialization(msg) => write!(f, "specialization failed: {msg}"),
            CodegenError::Realization(msg) => write!(f, "realization failed: {msg}"),
            CodegenError::Hugr(msg) => write!(f, "HUGR generation failed: {msg}"),
        }
    }
}

impl std::error::Error for CodegenError {}

impl From<isl::IslError> for CodegenError {
    fn from(e: isl::IslError) -> Self {
        CodegenError::Isl(e)
    }
}

pub type Result<T> = std::result::Result<T, CodegenError>;
