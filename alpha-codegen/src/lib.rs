//! `simpleC` model and the `WriteC` demand-driven C code generator.
//!
//! Cardinality/`malloc`-sizing via Barvinok is behind the optional, off-by-default `barvinok`
//! Cargo feature (GPL-licensed) — not wired up yet (`barvinok`/`barvinok-sys` are still stubs);
//! this crate's default (non-
//! `barvinok`) build uses a deliberately sanctioned isl-only bounding-box fallback instead, see
//! [`layout`]'s module doc.

pub mod error;
pub mod layout;
pub mod simplec;
pub mod writec;

pub use error::{CodegenError, Result};
pub use writec::generate_system;
