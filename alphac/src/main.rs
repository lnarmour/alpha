//! Alpha language CLI compiler.
//!
//! `alphac file.alpha -o file.c`, replacing the source project's `alpha.loader`, deliberately
//! without its accidental coupling to the schedule-tree grammar.
//!
//! `--wrapper` additionally emits a `*_wrapper.c` test harness per system (via
//! `alpha_codegen::generate_wrapper`) alongside `-o`'s own output file, in the same directory and
//! named after its stem — `-o dir/foo.c --wrapper` writes `dir/foo_wrapper.c` (or, for a file with
//! more than one system, one `dir/foo_<SystemName>_wrapper.c` per system). Requires `-o`: with no
//! output file there's no path to derive the wrapper's name/location from, so `--wrapper` alone is
//! a hard error.
//!
//! `--makefile` additionally emits a `Makefile` (via `alpha_codegen::generate_makefile`) next to
//! `-o`'s own output file, that compiles it to an object file — and, if `--wrapper` was also given,
//! links each wrapper against that object file into its own executable. Requires `-o` for the same
//! reason `--wrapper` does.
//!
//! Pipeline, per system found in the input file: parse (once, for the whole file) → analyze (all
//! six `alpha_model` phases, via `alpha_model::analyze_system`, the consolidated "run all of
//! alpha-model" entry point) → if clean, lower → `NormalizeReduction` → `Normalize`
//! → `alpha_codegen::generate_system` → print. This order is *required* by this port's own
//! `gen_value` (`alpha-codegen/src/expr.rs`), which hard-errors on a bare `Variable` node instead
//! of tolerating it as an implicit identity read the way upstream's `ExprConverter` does
//! (`convertExpr(VariableExpression)` in `ExprConverter.xtend`) — `NormalizeReduction` replaces
//! each hoisted `Reduce` with a bare `Variable` placeholder that only `Normalize`'s own
//! `ensure_variable_wrapped` step (run during its single top-down/bottom-up tree walk) will wrap
//! in an identity `Dependence`; running `Normalize` first would leave that placeholder unwrapped
//! forever. Confirmed by direct testing: swapping the order breaks codegen on every equation
//! containing a reduce (e.g. `PrefixScan.alpha`) with "internal error: bare Variable(...) reached
//! codegen outside a Dependence". Do not reorder this without also relaxing `gen_value`'s
//! Variable-arm to match upstream's more permissive handling. A file with more than one system
//! (nested `AlphaPackage`s included) gets one self-contained generated-C block per system,
//! concatenated — see the module doc on why each block carries its own preamble rather than
//! sharing one.

use alpha_syntax::ast;
use isl::Context;
use std::path::PathBuf;
use std::process::ExitCode;

fn all_systems(root: &ast::Root) -> Vec<ast::System> {
    let mut out: Vec<ast::System> = root.systems().collect();
    fn walk_pkg(pkg: &ast::AlphaPackage, out: &mut Vec<ast::System>) {
        out.extend(pkg.systems());
        for sub in pkg.packages() {
            walk_pkg(&sub, out);
        }
    }
    for pkg in root.packages() {
        walk_pkg(&pkg, &mut out);
    }
    out
}

struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
    wrapper: bool,
    makefile: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut input = None;
    let mut output = None;
    let mut wrapper = false;
    let mut makefile = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                output = Some(PathBuf::from(
                    args.next().ok_or("-o requires a path argument")?,
                ));
            }
            "--wrapper" => {
                wrapper = true;
            }
            "--makefile" => {
                makefile = true;
            }
            other if !other.starts_with('-') => {
                if input.is_some() {
                    return Err(format!("unexpected extra argument: {other}"));
                }
                input = Some(PathBuf::from(other));
            }
            other => return Err(format!("unrecognized option: {other}")),
        }
    }
    if wrapper && output.is_none() {
        return Err("--wrapper requires -o/--output to be specified".to_string());
    }
    if makefile && output.is_none() {
        return Err("--makefile requires -o/--output to be specified".to_string());
    }
    Ok(Args {
        input: input.ok_or("usage: alphac <file.alpha> [-o <file.c>] [--wrapper] [--makefile]")?,
        output,
        wrapper,
        makefile,
    })
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("alphac: {e}");
            return ExitCode::FAILURE;
        }
    };

    let src = match std::fs::read_to_string(&args.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("alphac: reading {:?}: {e}", args.input);
            return ExitCode::FAILURE;
        }
    };

    let parse = alpha_syntax::parse(&src);
    if !parse.errors.is_empty() {
        eprintln!("alphac: {} syntax error(s):", parse.errors.len());
        for e in &parse.errors {
            eprintln!("  {}..{}: {}", e.start, e.end, e.message);
        }
        return ExitCode::FAILURE;
    }
    let tree = parse.tree();
    let systems = all_systems(&tree);
    if systems.is_empty() {
        eprintln!("alphac: no systems found in {:?}", args.input);
        return ExitCode::FAILURE;
    }

    let mut had_diagnostics = false;
    let mut generated = Vec::new();
    let mut wrappers: Vec<(String, String)> = Vec::new();

    for system in &systems {
        let ctx = Context::new();
        let mut resolver = alpha_model::Resolver::new(ctx.clone(), system);
        let diagnostics = alpha_model::analyze_system(&mut resolver, system);
        if !diagnostics.is_empty() {
            had_diagnostics = true;
            let name = system
                .name()
                .map(|t| t.text().to_string())
                .unwrap_or_default();
            eprintln!(
                "alphac: {} diagnostic(s) in system '{name}':",
                diagnostics.len()
            );
            for d in &diagnostics {
                eprintln!("  {d}");
            }
            continue;
        }
        if had_diagnostics {
            continue;
        }

        let (mut ir_system, lower_diagnostics) =
            match alpha_transform::lower::lower_system(&mut resolver, system) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("alphac: internal error lowering system: {e}");
                    return ExitCode::FAILURE;
                }
            };
        for d in &lower_diagnostics {
            eprintln!("alphac: warning: equation skipped during lowering: {d}");
        }

        alpha_transform::normalize_reduction::apply(&mut ir_system);
        let normalized = alpha_transform::normalize::apply(ir_system, true);

        match alpha_codegen::generate_system(&normalized) {
            Ok(code) => generated.push(code),
            Err(e) => {
                eprintln!("alphac: codegen error: {e}");
                return ExitCode::FAILURE;
            }
        }

        if args.wrapper {
            match alpha_codegen::generate_wrapper(&normalized) {
                Ok(wrapper_code) => wrappers.push((normalized.name.clone(), wrapper_code)),
                Err(e) => {
                    eprintln!("alphac: wrapper codegen error: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    if had_diagnostics {
        return ExitCode::FAILURE;
    }

    let output_text = generated.join("\n");
    match &args.output {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &output_text) {
                eprintln!("alphac: writing {path:?}: {e}");
                return ExitCode::FAILURE;
            }
        }
        None => print!("{output_text}"),
    }

    let mut wrapper_paths: Vec<PathBuf> = Vec::new();
    if args.wrapper {
        // `parse_args` already rejected `--wrapper` without `-o`.
        let output_path = args.output.as_ref().expect("--wrapper requires -o");
        let stem = output_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let multi_system = wrappers.len() > 1;
        for (name, wrapper_code) in &wrappers {
            let file_name = if multi_system {
                format!("{stem}_{name}_wrapper.c")
            } else {
                format!("{stem}_wrapper.c")
            };
            let wrapper_path = output_path.with_file_name(file_name);
            if let Err(e) = std::fs::write(&wrapper_path, wrapper_code) {
                eprintln!("alphac: writing {wrapper_path:?}: {e}");
                return ExitCode::FAILURE;
            }
            wrapper_paths.push(wrapper_path);
        }
    }

    if args.makefile {
        // `parse_args` already rejected `--makefile` without `-o`.
        let output_path = args.output.as_ref().expect("--makefile requires -o");
        let makefile_text = alpha_codegen::generate_makefile(output_path, &wrapper_paths);
        let makefile_path = output_path.with_file_name("Makefile");
        if let Err(e) = std::fs::write(&makefile_path, makefile_text) {
            eprintln!("alphac: writing {makefile_path:?}: {e}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
