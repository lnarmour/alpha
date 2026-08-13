//! Generates a `*_wrapper.c`-style test harness for a system's public entry point (issue #23):
//! allocates memory for every parameter, calls the generated function, and frees it — so a
//! generated system can be compiled and run without hand-written boilerplate.
//!
//! Targets the exact driver signature both `crate::writec::generate_system` and
//! `crate::scheduledc::generate_scheduled_system` already produce (see `crate::layout`'s module
//! doc): scalar system parameters passed by value as `long`, every input/output passed as a
//! pointer chain whose depth equals its domain's dimensionality
//! (`layout::interface_ctype`) — so the storage this module builds must match that pointer depth
//! exactly to type-check, not just be "big enough". See `gen_array_alloc`'s doc for how.
//!
//! Array extents are generally symbolic in the system's own scalar parameters (e.g. `N`), so
//! concrete values for those parameters aren't known until the wrapper actually runs — `main`
//! reads them from `argv`, throwing an error when absent, matching the
//! reference AlphaZ wrapper's own convention of running the same binary across several sizes.

use crate::error::Result;
use crate::expr;
use crate::layout;
use crate::simplec::{write_stmts, CType, Expr as CExpr, Function, Stmt};
use alpha_transform::ir;
use isl::{DimType, Format, Set};
use std::fmt::Write as _;

/// Generates a self-contained wrapper source file for `system`'s public entry point.
pub fn generate_wrapper(system: &ir::System) -> Result<String> {
    let param_names = expr::param_names_of(&system.parameter_domain);
    let decl = Function {
        return_type: CType::Void,
        name: system.name.clone(),
        params: entry_params_list(system, &param_names),
        body: vec![],
        is_static: false,
    };

    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Auto-generated test wrapper for '{}' by alphac (alpha-rs).",
        system.name
    );
    let _ = writeln!(
        out,
        "// Allocates memory for each parameter, calls {}, and frees it.",
        system.name
    );
    let _ = writeln!(out);
    for inc in ["stdio.h", "stdlib.h"] {
        let _ = writeln!(out, "#include <{inc}>");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "{};", decl.signature());
    let _ = writeln!(out);

    let main_body = build_main_body(system, &param_names)?;
    let _ = writeln!(out, "int main(int argc, char **argv) {{");
    out.push_str(&render_body(&main_body));
    let _ = writeln!(out, "}}");

    Ok(out)
}

/// The entry point's own parameter list — mirrors `writec::gen_driver`/`scheduledc::build_driver`
/// exactly (scalar params as `long _local_<p>`, then every input/output as `_local_<name>` typed
/// via `layout::interface_ctype`), so the forward declaration this module emits always matches the
/// real entry point without needing a shared header file.
fn entry_params_list(system: &ir::System, param_names: &[String]) -> Vec<(CType, String)> {
    let mut params = Vec::new();
    for p in param_names {
        params.push((CType::Long, format!("_local_{p}")));
    }
    for v in system.inputs.iter().chain(system.outputs.iter()) {
        let ty = layout::interface_ctype(CType::Float, v.domain.dim(DimType::OutOrSet));
        params.push((ty, format!("_local_{}", v.name)));
    }
    params
}

fn build_main_body(system: &ir::System, param_names: &[String]) -> Result<Vec<Stmt>> {
    let mut body = Vec::new();

    body.push(Stmt::If {
        cond: CExpr::Raw(format!("argc != {}", param_names.len() + 1)),
        then_branch: vec![
            Stmt::Raw(
                "fprintf(stderr, \"alphac wrapper: wrong number of arguments\\n\");".to_string(),
            ),
            usage_hint(param_names),
            Stmt::Return(Some(CExpr::Raw("1".to_string()))),
        ],
        else_branch: vec![],
    });

    body.push(Stmt::Decl {
        ty: CType::Ptr(Box::new(CType::Char), 1),
        name: "endptr".to_string(),
        init: None,
    });

    for (i, p) in param_names.iter().enumerate() {
        let argn = i + 1;
        body.push(Stmt::Decl {
            ty: CType::Long,
            name: p.clone(),
            init: Some(CExpr::Raw(format!("strtol(argv[{argn}], &endptr, 10)"))),
        });

        body.push(Stmt::If {
            cond: CExpr::Raw("*endptr != '\\0'".to_string()), 
            then_branch: vec![
                Stmt::Raw(format!(
                    "fprintf(stderr, \"alphac wrapper: could not convert argument {argn} (%s) to long.\\n\", argv[{argn}]);"
                )),
                Stmt::Return(Some(CExpr::Raw("1".to_string()))),
            ],
            else_branch: vec![],
        });
    }
    if !param_names.is_empty() {
        body.push(Stmt::Raw(String::new()));
    }

    // Shared loop-iterator local, reused by every (sequential, never-nested) wiring/fill loop
    // below — safe since no two such loops are ever active at once.
    body.push(Stmt::Decl {
        ty: CType::Long,
        name: "_i".to_string(),
        init: None,
    });
    body.push(Stmt::Raw(String::new()));

    let mut call_args: Vec<CExpr> = param_names.iter().map(|p| CExpr::Raw(p.clone())).collect();
    let mut free_stmts: Vec<Stmt> = Vec::new();

    for (v, is_input) in system
        .inputs
        .iter()
        .map(|v| (v, true))
        .chain(system.outputs.iter().map(|v| (v, false)))
    {
        let (alloc, frees, top_name) = gen_array_alloc(v, is_input)?;
        body.extend(alloc);
        body.push(Stmt::Raw(String::new()));
        free_stmts.extend(frees);
        call_args.push(CExpr::Raw(top_name));
    }

    body.push(Stmt::Expr(CExpr::Call(system.name.clone(), call_args)));
    body.push(Stmt::Raw(String::new()));
    body.push(Stmt::Raw(format!(
        "printf(\"{}: allocated, called, and freed successfully.\\n\");",
        system.name
    )));
    body.push(Stmt::Raw(String::new()));
    body.extend(free_stmts);
    body.push(Stmt::Return(Some(CExpr::Raw("0".to_string()))));

    Ok(body)
}

/// Returns a printf statement that tells the user the wrapper's proper usage.
fn usage_hint(param_names: &[String]) -> Stmt {
    let params = param_names
        .iter()
        .map(|p| format!("[{p}]"))
        .collect::<Vec<String>>()
        .join(" ");

    Stmt::Raw(format!(
        "printf(\"alphac wrapper: Usage: %s {params}\\n\", argv[0]);"
    ))
}

/// Per-dimension `dim_max(d) + 1`, as C text — interface arrays are indexed directly with no
/// offset (`layout` module doc), so allocation must cover index `0..=dim_max(d)` regardless of
/// the domain's actual (possibly non-rectangular) shape, exactly like the reference wrapper's own
/// dense-block-regardless-of-domain-shape convention. A 0-D domain (no dimensions to loop over)
/// gets a single synthetic extent of `1`, matching `interface_ctype`'s forced minimum of one
/// pointer level.
fn dim_extents(domain: &Set, ndims: u32) -> Result<Vec<String>> {
    if ndims == 0 {
        return Ok(vec!["1".to_string()]);
    }
    (0..ndims)
        .map(|d| {
            Ok(format!(
                "(({}) + 1)",
                domain.dim_max(d)?.to_string_fmt(Format::C)
            ))
        })
        .collect()
}

fn extents_product(exts: &[String]) -> String {
    if exts.is_empty() {
        "1".to_string()
    } else {
        format!("({})", exts.join(" * "))
    }
}

/// `var_name` itself for the topmost level (the value actually passed to the call); a synthetic,
/// collision-free name for every level underneath it.
fn level_name(var_name: &str, level: u32, top_level: u32) -> String {
    if level == top_level {
        var_name.to_string()
    } else {
        format!("_{var_name}_l{level}")
    }
}

fn alloc_check_stmt(var_name: &str, source_var: &str) -> Stmt {
    Stmt::If {
        cond: CExpr::Raw(format!("{var_name} == NULL")),
        then_branch: vec![
            Stmt::Raw(format!(
                "fprintf(stderr, \"alphac wrapper: failed to allocate memory for '{source_var}'\\n\");"
            )),
            Stmt::Return(Some(CExpr::Raw("1".to_string()))),
        ],
        else_branch: vec![],
    }
}

/// Allocates a `k`-dimensional interface variable exactly matching its own entry-point parameter
/// type (`layout::interface_ctype`: `k` levels of `*`, e.g. `float**` for 2-D) — one flat data
/// buffer plus `k - 1` pointer-array levels wired on top of it. `X[i0][i1]...[i(k-1)]` on a
/// genuinely `k`-deep pointer chain dereferences one real pointer per `[]`, so each of those `k-1`
/// levels must be a real, separately allocated array of pointers into the level below it — this is
/// a direct generalization of the reference wrapper's own 2-D pattern (one flat `malloc` plus one
/// row-pointer `malloc`, wired by a single loop: `for (i=0;i<N;i++) A[i] = &_lin_A[i*N];`) to `k-1`
/// such loops instead of just one; `k <= 2` reduces to exactly that existing shape.
///
/// Returns the allocation statements, the statements that free every level, and the name of the
/// topmost variable — the value to pass to the call.
fn gen_array_alloc(v: &ir::Variable, is_input: bool) -> Result<(Vec<Stmt>, Vec<Stmt>, String)> {
    let ndims = v.domain.dim(DimType::OutOrSet);
    let extents = dim_extents(&v.domain, ndims)?;
    let k = extents.len() as u32;
    let top = k - 1;

    let mut stmts = Vec::new();
    let mut frees = Vec::new();

    // Level 0: the flat data buffer — the only level that ever holds real float values.
    let data_name = level_name(&v.name, 0, top);
    let total = extents_product(&extents);
    let alloc_expr = if is_input {
        format!("(float*)malloc(sizeof(float) * {total})")
    } else {
        format!("(float*)calloc({total}, sizeof(float))")
    };
    stmts.push(Stmt::Decl {
        ty: CType::ptr(CType::Float, 1),
        name: data_name.clone(),
        init: Some(CExpr::Raw(alloc_expr)),
    });
    stmts.push(alloc_check_stmt(&data_name, &v.name));
    if is_input {
        stmts.push(Stmt::For {
            iterator: "_i".to_string(),
            init: "0".to_string(),
            cond: format!("_i < {total}"),
            inc: "1".to_string(),
            body: vec![Stmt::Assign {
                target: CExpr::Raw(format!("{data_name}[_i]")),
                value: CExpr::Raw("(float)(_i % 1009) + 1.0f".to_string()),
            }],
        });
    }
    frees.push(Stmt::Raw(format!("free({data_name});")));

    // Levels 1..=top: pointer-array levels wired on top of the previous level, one loop each.
    let mut prev_name = data_name;
    for level in 1..=top {
        let size = extents_product(&extents[0..(k - level) as usize]);
        let stride = &extents[(k - level) as usize];
        let this_ctype = CType::ptr(CType::Float, level + 1);
        let prev_ctype = CType::ptr(CType::Float, level);
        let this_name = level_name(&v.name, level, top);
        stmts.push(Stmt::Decl {
            ty: this_ctype.clone(),
            name: this_name.clone(),
            init: Some(CExpr::Raw(format!(
                "({this_ctype})malloc(sizeof({prev_ctype}) * {size})"
            ))),
        });
        stmts.push(alloc_check_stmt(&this_name, &v.name));
        stmts.push(Stmt::For {
            iterator: "_i".to_string(),
            init: "0".to_string(),
            cond: format!("_i < {size}"),
            inc: "1".to_string(),
            body: vec![Stmt::Assign {
                target: CExpr::Raw(format!("{this_name}[_i]")),
                value: CExpr::Raw(format!("&{prev_name}[_i * {stride}]")),
            }],
        });
        frees.push(Stmt::Raw(format!("free({this_name});")));
        prev_name = this_name;
    }

    Ok((stmts, frees, prev_name))
}

/// Renders a statement list at the standard single-level-deep body indentation — the same helper
/// [`Function`]'s own `Display` impl uses internally, exposed for callers (e.g. `crate::wrapper`)
/// that need a function body's text without a `Function`'s signature line (this crate's `CType`
/// has no `Int` variant, so `int main(...)` can't be built as a `Function` directly).
fn render_body(stmts: &[Stmt]) -> String {
    let mut out = String::new();
    write_stmts(&mut out, stmts, 1);
    out
}
