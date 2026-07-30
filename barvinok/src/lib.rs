//! Safe Rust wrapper over `barvinok-sys`.
//!
//! GPL-licensed — see this crate's `Cargo.toml` and `docs/rust-port-design.md` §5. Only
//! `alpha-codegen`'s optional `barvinok` Cargo feature may depend on this crate; the default
//! `alphac` build and the VS Code native addon must never pull it in.
