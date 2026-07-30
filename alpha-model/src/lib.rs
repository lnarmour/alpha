//! Semantic model and six-phase checker for the Alpha language.
//!
//! See `docs/rust-port-design.md` §6 in the workspace root: interface/expression domain
//! resolution, expression-domain / context-domain inference, name-uniqueness, and the
//! uniqueness-and-completeness checks, all reported through a closed `Diagnostic` enum.
//!
//! Implemented so far: phase 1 (interface resolution — system parameter domains, variable
//! domains, `define`d objects, `RectangularDomain` expansion) in [`resolve`], the
//! calculator-expression evaluator ("the calculator's tiny type system") in [`value`], and
//! (part of phase 2) `Function`/`ArrayFunction` → `MultiAff` resolution in [`function`]. The
//! rest of phase 2 (threading ambient index-name context through full equation bodies) and
//! phases 3–6 (expression/context domain inference, name uniqueness, well-formedness) are not
//! yet implemented.

pub mod diagnostic;
pub mod function;
pub mod resolve;
pub mod value;

pub use diagnostic::Diagnostic;
pub use resolve::Resolver;
pub use value::Value;
