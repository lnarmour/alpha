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
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodegenError::Isl(e) => write!(f, "isl error: {e}"),
            CodegenError::Unsupported(msg) => write!(f, "unsupported: {msg}"),
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
