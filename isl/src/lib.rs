//! Safe, idiomatic Rust wrapper over `isl-sys`.
//!
//! Covers a bounded operation inventory (a real subset of full isl, not "all of it"): sets/maps,
//! affine functions, constraints, piecewise quasipolynomials, and the AST builder. MIT-licensed;
//! must never depend on `barvinok` (GPL) — cardinality/Ehrhart counting lives there instead,
//! feature-gated in `alpha-codegen`.
//!
//! Ownership: isl's C API is consumption-oriented (most operations free their inputs and return
//! a new object). This crate mirrors that in Rust's type system directly — every isl call that
//! consumes its argument(s) takes `self`/the argument by value here, so the borrow checker
//! enforces "you can't reuse an isl object after an operation that consumed it" at compile time,
//! and `Clone` (backed by isl's real `_copy` functions — cheap, since isl objects are internally
//! refcounted/copy-on-write already) is the only way to reuse a value across two operations.
//! Every fallible call returns `Result<T, IslError>`, converting isl's own null-on-error
//! convention via `Context::check`.

mod aff;
mod ast;
mod constraint;
mod ctx;
mod map;
mod polynomial;
mod set;
mod space;

pub use aff::{Aff, MultiAff, PwAff};
pub use ast::{AstBuild, AstExpr, AstExprKind, AstNode, AstNodeKind, UnionMap};
pub use constraint::{Constraint, LocalSpace};
pub use ctx::{Context, IslError, Result};
pub use map::Map;
pub use polynomial::PwQPolynomial;
pub use set::{BasicSet, Format, Set};
pub use space::{DimType, Space};

/// Re-exported for callers that need isl operator-kind constants directly (e.g. matching
/// [`AstExpr::op_type`]) without depending on `isl-sys` themselves.
pub use isl_sys::isl_ast_expr_op_type as AstOpType;
