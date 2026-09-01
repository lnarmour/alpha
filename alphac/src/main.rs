//! Alpha language CLI compiler.
//!
//! `alphac file.alpha -o file.c`, replacing the source project's `alpha.loader`, deliberately
//! without its accidental coupling to the schedule-tree grammar.
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
use std::collections::BTreeMap;
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Emit {
    C,
    Hugr,
}

struct Args {
    input: PathBuf,
    output: Option<PathBuf>,
    emit: Emit,
    schedule: Option<PathBuf>,
    parameters: BTreeMap<String, i64>,
}

fn parse_args() -> Result<Args, String> {
    let mut input = None;
    let mut output = None;
    let mut emit = Emit::C;
    let mut schedule = None;
    let mut parameters = BTreeMap::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--output" => {
                output = Some(PathBuf::from(
                    args.next().ok_or("-o requires a path argument")?,
                ));
            }
            "--emit" => {
                emit = match args.next().as_deref() {
                    Some("c") => Emit::C,
                    Some("hugr") => Emit::Hugr,
                    Some(value) => return Err(format!("unknown emission format '{value}'")),
                    None => return Err("--emit requires c or hugr".to_string()),
                };
            }
            "--schedule" => {
                schedule = Some(PathBuf::from(
                    args.next().ok_or("--schedule requires a path argument")?,
                ));
            }
            "--param" => {
                let binding = args.next().ok_or("--param requires NAME=VALUE")?;
                let (name, value) = binding
                    .split_once('=')
                    .filter(|(name, value)| !name.is_empty() && !value.is_empty())
                    .ok_or_else(|| {
                        format!("malformed parameter '{binding}', expected NAME=VALUE")
                    })?;
                let value = value.parse::<i64>().map_err(|_| {
                    format!("invalid value in parameter '{binding}', expected an integer")
                })?;
                if parameters.insert(name.to_string(), value).is_some() {
                    return Err(format!("duplicate parameter '{name}'"));
                }
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
    Ok(Args {
        input: input.ok_or(
            "usage: alphac [--emit c|hugr] [--schedule file.isl] [--param NAME=VALUE] <file.alpha> [-o <output>]",
        )?,
        output,
        emit,
        schedule,
        parameters,
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
    let schedule_text = match &args.schedule {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("alphac: reading schedule {path:?}: {error}");
                return ExitCode::FAILURE;
            }
        },
        None => String::new(),
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

        let result = match args.emit {
            Emit::C => alpha_codegen::generate_system(&normalized),
            Emit::Hugr => {
                alpha_codegen::generate_hugr_system(&normalized, &schedule_text, &args.parameters)
            }
        };
        match result {
            Ok(code) => generated.push(code),
            Err(e) => {
                eprintln!("alphac: codegen error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if had_diagnostics {
        return ExitCode::FAILURE;
    }

    let output_text = generated.join("\n");
    match args.output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, output_text) {
                eprintln!("alphac: writing {path:?}: {e}");
                return ExitCode::FAILURE;
            }
        }
        None => print!("{output_text}"),
    }

    ExitCode::SUCCESS
}
