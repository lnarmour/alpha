use alpha_model::Resolver;
use alpha_syntax::ast;
use isl::Context;
use std::path::{Path, PathBuf};

pub(crate) fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/alpha-language-fixtures")
}

/// Parses/analyzes/lowers/normalizes the first system in `src` — panics on any front-end failure
/// (these are hand-written fixtures, expected to be clean), mirroring
/// `scheduledc_e2e.rs`'s own `lowered_prefix_sum` helper, generalized to any source text.
pub(crate) fn lowered_system(src: &str) -> alpha_transform::ir::System {
    let ctx = Context::new();
    let parse = alpha_syntax::parse(src);
    assert!(parse.errors.is_empty(), "{:?}", parse.errors);
    let tree = parse.tree();
    let system = tree.systems().next().expect("one system in fixture");
    let mut resolver = Resolver::new(ctx, &system);
    let diagnostics = alpha_model::analyze_system(&mut resolver, &system);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let (mut ir_system, lower_diagnostics) =
        alpha_transform::lower::lower_system(&mut resolver, &system).unwrap();
    assert!(lower_diagnostics.is_empty(), "{lower_diagnostics:?}");
    alpha_transform::normalize_reduction::apply(&mut ir_system);
    alpha_transform::normalize::apply(ir_system, true)
}

pub(crate) fn all_systems(root: &ast::Root) -> Vec<ast::System> {
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

/// A documented, deliberate codegen scope boundary this session doesn't implement — see
/// `writec`'s module doc. Recognized by a short, stable substring of the error message.
pub(crate) fn is_known_scope_boundary(msg: &str) -> bool {
    [
        "UseEquation has no codegen backend",
        "select {relation} from E codegen not implemented",
        "val{...} (polynomial-valued index expression) codegen not implemented",
        "argreduce codegen not implemented",
        "val(f) codegen only implemented for a single-output function",
        "Convolution codegen not implemented",
        // Not a bug: a system with no `let` block at all has nothing to generate.
        "system has no bodies to generate code for",
        // A real, documented limitation of this session's from-context bound derivation, not a
        // crash — see `isl_bound_err`'s doc in `writec.rs`.
        "isl couldn't establish a bound",
    ]
    .iter()
    .any(|known| msg.contains(known))
}

pub(crate) fn all_alpha_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {dir:?}: {e}")) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            all_alpha_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "alpha") {
            out.push(path);
        }
    }
}
