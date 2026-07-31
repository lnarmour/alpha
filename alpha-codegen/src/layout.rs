//! Storage layout for a system's variables — where a value with a given (possibly parametric)
//! domain actually lives in memory, and how to index into it.
//!
//! Two different schemes, matching what's actually observed in the source project's own generated
//! C (`~/git/poly/alpha-language/tests/alpha.codegen.tests/resources/{CopyInput,PrefixScan,
//! LUDecomposition}/*.c`, read directly as reference output for this port — see
//! `docs/rust-port-design.md` §7):
//!
//! - **Interface variables** (a system's own `inputs`/`outputs`): a plain pointer chain (`float*`
//!   for a 1-D domain, `float**` for 2-D, ...) indexed *directly* by the equation's own index
//!   values with no offset adjustment at all, even when the domain doesn't start at 0 (e.g.
//!   `LUDecomposition`'s `L: {[i,j]: 0<i<N and 0<=j<i}` still indexes as plain `L[i][j]`) — memory
//!   for these arrays is the *caller's* responsibility (see the `*-wrapper.c` files: they allocate
//!   a full dense `N`-by-`N` block regardless of the triangular domain actually used), so this
//!   port doesn't need to size or offset them at all.
//! - **Flat variables** (locals, and every generated flag array — including one for each output,
//!   which needs memoization too): a single 1-D allocation this generator sizes and frees itself,
//!   indexed via a row-major linearization over a **bounding box** of the domain (each dimension's
//!   own min/max via [`isl::Set::dim_min`]/[`isl::Set::dim_max`], computed independently per
//!   dimension). This is the sanctioned isl-only fallback `docs/rust-port-design.md` §5 calls out
//!   explicitly ("conservative overallocation... for the box/rectangular domain cases that cover
//!   the vast majority of real programs... falling back to conservative overallocation... when the
//!   domain isn't simple") rather than the source system's own exact, Barvinok/Ehrhart-derived
//!   linearization (visible in its reference output as the piecewise `(i,j) -> rank` formulas on
//!   `U_NR`/`L_NR`/every flag array in `LUDecomposition.c`) — implementing that exactly is real,
//!   separate work gated behind the (currently stub) `barvinok` feature, not attempted this
//!   session. For a rectangular domain (`CopyInput`, `PrefixScan`) the two coincide exactly; for a
//!   triangular one (`LUDecomposition`) this allocates strictly more than the source system does,
//!   never less, so it stays correct.

use crate::error::Result;
use crate::simplec::CType;
use isl::{DimType, Format, Set};

pub struct FlatBounds {
    /// One C expression per dimension, computed once (into a `static long` global) and reused
    /// wherever that dimension's low/high bound is needed.
    pub mins: Vec<String>,
    pub maxs: Vec<String>,
}

impl FlatBounds {
    pub fn compute(domain: &Set) -> Result<FlatBounds> {
        let n = domain.dim(DimType::OutOrSet);
        let mut mins = Vec::with_capacity(n as usize);
        let mut maxs = Vec::with_capacity(n as usize);
        for d in 0..n {
            mins.push(domain.dim_min(d)?.to_string_fmt(Format::C));
            maxs.push(domain.dim_max(d)?.to_string_fmt(Format::C));
        }
        Ok(FlatBounds { mins, maxs })
    }
}

/// A flat variable's generated global names: `<name>_min<d>`/`<name>_size<d>` per dimension.
pub fn min_global(name: &str, d: u32) -> String {
    format!("{name}_min{d}")
}
pub fn size_global(name: &str, d: u32) -> String {
    format!("{name}_size{d}")
}

/// `static long` declarations for a flat variable's per-dimension min/size globals (empty for a
/// 0-D domain — there's nothing to size per-dimension, storage is just one cell).
pub fn flat_size_decls(name: &str, ndims: u32) -> Vec<String> {
    let mut out = Vec::new();
    for d in 0..ndims {
        out.push(format!("static long {};", min_global(name, d)));
        out.push(format!("static long {};", size_global(name, d)));
    }
    out
}

/// Assignment statements computing a flat variable's min/size globals from its domain's bounding
/// box — emitted once, in the entry function, before that variable's `malloc`.
pub fn flat_size_init_stmts(name: &str, bounds: &FlatBounds) -> Vec<String> {
    let mut out = Vec::new();
    for (d, (min, max)) in bounds.mins.iter().zip(bounds.maxs.iter()).enumerate() {
        let d = d as u32;
        out.push(format!("{} = {};", min_global(name, d), min));
        out.push(format!(
            "{} = max(0, ({}) - ({}) + 1);",
            size_global(name, d),
            max,
            min_global(name, d)
        ));
    }
    out
}

/// The total element count of a flat variable's storage — the product of every dimension's size
/// global (`1` for a 0-D domain: a single scalar cell).
pub fn flat_total_size_expr(name: &str, ndims: u32) -> String {
    if ndims == 0 {
        return "1".to_string();
    }
    (0..ndims)
        .map(|d| size_global(name, d))
        .collect::<Vec<_>>()
        .join(" * ")
}

/// The row-major flat-index expression for a flat variable, given its own index-variable names
/// (`idx[d]` is whatever C expression is being used as dimension `d`'s value — usually a bare
/// identifier, but callers may pass a more complex expression, e.g. a composed dependence-function
/// component).
pub fn flat_index_expr(name: &str, idx: &[String]) -> String {
    let ndims = idx.len() as u32;
    if ndims == 0 {
        return "0".to_string();
    }
    let mut terms = Vec::new();
    for d in 0..ndims {
        let stride = if d + 1 == ndims {
            "1".to_string()
        } else {
            (d + 1..ndims)
                .map(|e| size_global(name, e))
                .collect::<Vec<_>>()
                .join(" * ")
        };
        terms.push(format!(
            "(({}) - ({})) * ({})",
            idx[d as usize],
            min_global(name, d),
            stride
        ));
    }
    terms.join(" + ")
}

/// `#define NAME(i0,i1,...) NAME[<flat index>]` for a flat (generator-owned) variable.
pub fn flat_access_macro(name: &str, ndims: u32) -> String {
    let params: Vec<String> = (0..ndims).map(|d| format!("i{d}")).collect();
    let idx = flat_index_expr(name, &params);
    if ndims == 0 {
        format!("#define {name}() {name}[0]")
    } else {
        format!("#define {name}({}) {name}[{idx}]", params.join(","))
    }
}

/// `#define NAME(i0,i1,...) NAME[i0][i1]...` for an interface (caller-owned) variable — direct,
/// unadjusted pointer-chain indexing (see the module doc for why no offset is applied).
pub fn interface_access_macro(name: &str, ndims: u32) -> String {
    if ndims == 0 {
        return format!("#define {name}() {name}[0]");
    }
    let params: Vec<String> = (0..ndims).map(|d| format!("i{d}")).collect();
    let chain: String = params.iter().map(|p| format!("[{p}]")).collect();
    format!("#define {name}({}) {name}{chain}", params.join(","))
}

pub fn interface_ctype(elem: CType, ndims: u32) -> CType {
    CType::ptr(elem, ndims.max(1))
}
