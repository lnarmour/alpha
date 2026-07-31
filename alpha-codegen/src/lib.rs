//! `simpleC` model and the `WriteC` demand-driven C code generator.
//!
//! See `docs/rust-port-design.md` §7 in the workspace root. Cardinality/`malloc`-sizing via
//! Barvinok is behind the optional, off-by-default `barvinok` Cargo feature (GPL — §5, §10) —
//! not wired up yet (`barvinok`/`barvinok-sys` are still stubs); this crate's default (non-
//! `barvinok`) build uses the isl-only bounding-box fallback §5 explicitly sanctions instead, see
//! [`layout`]'s module doc.

pub mod error;
pub mod layout;
pub mod simplec;
pub mod writec;

pub use error::{CodegenError, Result};
pub use writec::generate_system;
