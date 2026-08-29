//! `simpleC` model and the `WriteC` demand-driven C code generator.
//!
//! Cardinality/`malloc`-sizing via Barvinok is behind the optional, off-by-default `barvinok`
//! Cargo feature (GPL-licensed) — not wired up yet (`barvinok`/`barvinok-sys` are still stubs);
//! this crate's default (non-
//! `barvinok`) build uses a deliberately sanctioned isl-only bounding-box fallback instead, see
//! [`layout`]'s module doc.

pub mod describe;
pub mod error;
mod expr;
pub mod hugr;
pub mod layout;
mod legality;
pub mod realize;
mod schedule;
pub mod scheduled_ir;
mod scheduledc;
pub mod simplec;
pub mod specialize;
pub mod stmt;
#[cfg(test)]
mod test_util;
pub mod writec;

pub use describe::{describe_normalized_system, describe_system};
pub use error::{CodegenError, Result};
pub use hugr::{generate_hugr, generate_hugr_system};
pub use scheduledc::{generate_scheduled_system, validate_scheduled_system};
pub use writec::generate_system;
