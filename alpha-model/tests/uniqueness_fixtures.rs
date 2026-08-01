//! Phase 5 conformance: every real fixture in the corpus is expected to have zero name-duplicate
//! diagnostics (none of them deliberately test this — see the unit tests below for that), so
//! `check_system_uniqueness` running clean over the whole corpus is itself the regression check.
//! Focused unit tests (hand-written, not fixture-based) cover the actual duplicate-detection
//! logic, since no fixture in the corpus is dedicated to it (checked: none under `src-invalid`
//! test name-duplication specifically).

use alpha_model::{check_program_uniqueness, check_system_uniqueness, Diagnostic};
use alpha_syntax::ast::{self};
use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../alpha-language/tests")
}

fn all_alpha_files(dir: &Path, out: &mut Vec<PathBuf>) {
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

#[test]
fn no_real_fixture_has_duplicate_names() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut n_systems = 0usize;
    let mut failures = Vec::new();

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parse = alpha_syntax::parse(&src);
        if !parse.errors.is_empty() {
            continue; // covered by alpha-syntax's own fixture tests
        }
        let tree = parse.tree();

        for system in all_systems(&tree) {
            n_systems += 1;
            let diags = check_system_uniqueness(&system);
            if !diags.is_empty() {
                failures.push((path.clone(), diags));
            }
        }
    }

    eprintln!(
        "checked name-uniqueness for {n_systems} systems across {} fixtures",
        files.len()
    );
    assert!(n_systems > 0, "found zero systems across the whole corpus");
    assert!(
        failures.is_empty(),
        "{} fixtures unexpectedly reported duplicate names:\n{:#?}",
        failures.len(),
        failures
    );
}

fn parse_ok(src: &str) -> ast::Root {
    let parse = alpha_syntax::parse(src);
    assert!(
        parse.errors.is_empty(),
        "unexpected syntax errors: {:?}",
        parse.errors
    );
    parse.tree()
}

#[test]
fn duplicate_variable_names_are_flagged() {
    let root = parse_ok(
        "affine System [N]->{:N>0}\n\
         \tinputs\n\
         \t\tX : [N]\n\
         \toutputs\n\
         \t\tX : [N]\n\
         \tlet\n\
         \t\tX = X;\n\
         .\n",
    );
    let system = root.systems().next().expect("one system");
    let diags = check_system_uniqueness(&system);
    let dup_vars: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d, Diagnostic::DuplicateVariable { name, .. } if name == "X"))
        .collect();
    assert_eq!(
        dup_vars.len(),
        2,
        "expected both X declarations flagged: {diags:#?}"
    );
}

#[test]
fn duplicate_between_variable_and_defined_object_is_flagged() {
    let root = parse_ok(
        "affine System [N]->{:N>0}\n\
         \tdefine X = {[i]:0<=i<N}\n\
         \tinputs\n\
         \t\tA : [N]\n\
         \toutputs\n\
         \t\tX : [N]\n\
         \tlet\n\
         \t\tX = A;\n\
         .\n",
    );
    let system = root.systems().next().expect("one system");
    let diags = check_system_uniqueness(&system);
    let dup_x: Vec<_> = diags
        .iter()
        .filter(|d| {
            matches!(
                d,
                Diagnostic::DuplicateVariable { name, .. }
                | Diagnostic::DuplicatePolyhedralObject { name, .. }
                if name == "X"
            )
        })
        .collect();
    assert_eq!(
        dup_x.len(),
        2,
        "expected both X declarations flagged: {diags:#?}"
    );
}

#[test]
fn duplicate_standard_equation_target_is_flagged() {
    let root = parse_ok(
        "affine System [N]->{:N>0}\n\
         inputs\n\
         \tA : [N]\n\
         outputs\n\
         \tB : [N]\n\
         let\n\
         \tB = A;\n\
         \tB = A;\n\
         .\n",
    );
    let system = root.systems().next().expect("one system");
    let diags = check_system_uniqueness(&system);
    let dup_eqs: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d, Diagnostic::DuplicateStandardEquation { name, .. } if name == "B"))
        .collect();
    assert_eq!(
        dup_eqs.len(),
        2,
        "expected both B equations flagged: {diags:#?}"
    );
}

#[test]
fn use_equations_alone_writing_to_the_same_variable_are_not_flagged() {
    // Two UseEquations targeting the same output variable are legal on their own (disjoint
    // instantiation domains are checked elsewhere, not here) — matches the source system's
    // "skip conflicts only within UseEquations" rule.
    let root = parse_ok(
        "affine System [N]->{:N>0}\n\
         inputs\n\
         \tA : [N]\n\
         outputs\n\
         \tB : [N]\n\
         let\n\
         \t(B) = Sub[N](A);\n\
         \t(B) = Sub[N](A);\n\
         .\n",
    );
    let system = root.systems().next().expect("one system");
    let diags = check_system_uniqueness(&system);
    assert!(
        diags.is_empty(),
        "two UseEquations alone should not conflict: {diags:#?}"
    );
}

#[test]
fn duplicate_constant_in_direct_container_is_flagged() {
    let root = parse_ok(
        "package tests.dup {\n\
         \tconstant K = 1\n\
         \tconstant K = 2\n\
         \taffine System [N]->{:N>0}\n\
         \t\tinputs\n\
         \t\t\tA : [N]\n\
         \t\toutputs\n\
         \t\t\tB : [N]\n\
         \t\tlet\n\
         \t\t\tB = A;\n\
         \t.\n\
         }\n",
    );
    let pkg = root.packages().next().expect("one package");
    let system = pkg.systems().next().expect("one system");
    let diags = check_system_uniqueness(&system);
    let dup_consts: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d, Diagnostic::DuplicateAlphaConstant { name, .. } if name == "K"))
        .collect();
    assert_eq!(
        dup_consts.len(),
        2,
        "expected both K constants flagged: {diags:#?}"
    );
}

#[test]
fn duplicate_system_across_roots_is_flagged() {
    let src = "affine System [N]->{:N>0}\n\
               inputs\n\
               \tA : [N]\n\
               outputs\n\
               \tB : [N]\n\
               let\n\
               \tB = A;\n\
               .\n";
    let root_a = parse_ok(src);
    let root_b = parse_ok(src);
    let diags = check_program_uniqueness(&[root_a, root_b]);
    let dup_systems: Vec<_> = diags
        .iter()
        .filter(|d| matches!(d, Diagnostic::DuplicateSystem { name, .. } if name == "System"))
        .collect();
    assert_eq!(
        dup_systems.len(),
        2,
        "expected both System decls flagged: {diags:#?}"
    );
}
