//! `simpleC` model and the `WriteC` demand-driven C code generator.
//!
//! See `docs/rust-port-design.md` §7 in the workspace root. Cardinality/`malloc`-sizing via
//! Barvinok is behind the optional, off-by-default `barvinok` Cargo feature (GPL — §5, §10).
