//! Semantic model and six-phase checker for the Alpha language.
//!
//! Interface/expression domain resolution, expression-domain / context-domain inference,
//! name-uniqueness, and the uniqueness-and-completeness checks, all reported through a closed
//! `Diagnostic` enum.
//!
//! Implemented so far: phase 1 (interface resolution — system parameter domains, variable
//! domains, `define`d objects, `RectangularDomain` expansion) in [`resolve`], the
//! calculator-expression evaluator ("the calculator's tiny type system") in [`value`],
//! `Function`/`ArrayFunction` → `MultiAff` resolution in [`function`] (part of phase 2), phases
//! 3–4 (expression-domain / context-domain inference) in [`domain`], phase 5 (name uniqueness) in
//! [`uniqueness`], and phase 6 (the well-formedness catalog) in [`completeness`]. All six phases
//! now exist, with three documented, deliberate scope
//! boundaries — see [`domain`]'s module doc (convolution's own domain, `UseEquation`'s context
//! domain) and [`completeness`]'s module doc (self-recursion detection by bare name, and the
//! `UseEquation`-output-completeness check that stays dormant until `domain`'s gap closes).

pub mod analyze;
pub mod check;
pub mod completeness;
pub mod context_names;
pub mod diagnostic;
pub mod domain;
pub mod function;
pub mod multiplicity;
pub mod resolve;
pub mod uniqueness;
pub mod value;
pub mod walk;

pub use analyze::{analyze_root, analyze_system};
pub use check::check_source;
pub use diagnostic::Diagnostic;
pub use domain::Domains;
pub use multiplicity::{builtin_signature, Multiplicity, PortSignature, VariableId};
pub use resolve::Resolver;
pub use uniqueness::{check_program_uniqueness, check_system_uniqueness};
pub use value::Value;
