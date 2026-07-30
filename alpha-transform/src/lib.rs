//! `Normalize` and `NormalizeReduction`: the only two transformation passes the demand-driven
//! codegen path depends on. See `docs/rust-port-design.md` §7 in the workspace root. The rest of
//! the source project's transformation/scheduling family (tiling, memory-mapping, reduction
//! simplification search, ...) is out of scope for this port.
