//! `Normalize` and `NormalizeReduction`: the only two transformation passes the demand-driven
//! codegen path depends on. See `docs/rust-port-design.md` §7 in the workspace root. The rest of
//! the source project's transformation/scheduling family (tiling, memory-mapping, reduction
//! simplification search, ...) is out of scope for this port.
//!
//! [`ir`] defines the owned, mutable tree these passes rewrite (distinct from
//! `alpha_syntax::ast`'s lossless CST — see that module's doc for why); [`lower`] builds one from
//! an analyzed `alpha_syntax::ast::System`; [`normalize`] and [`normalize_reduction`] are the two
//! passes themselves.

pub mod ir;
pub mod lower;
pub mod normalize;
pub mod normalize_reduction;
