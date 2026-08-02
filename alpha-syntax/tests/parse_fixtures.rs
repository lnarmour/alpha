//! Parser conformance check against the real `.alpha` corpus from the sibling `alpha-language`
//! (Java/Xtext) repo — the milestone-1 acceptance bar for this port.
//!
//! All 82 fixtures under `tests/**` — `src-valid` *and* `src-invalid` alike — are, in fact,
//! syntactically well-formed Alpha: despite one `src-invalid` subdirectory being named
//! `syntax-tests`, every fixture there (confirmed by reading them) tests a *semantic* violation
//! (dimension mismatches, duplicate/incomplete equation definitions, recursive subsystem calls,
//! ...) that the grammar itself accepts fine — semantic analysis (`alpha-model`, not yet
//! implemented) is what's expected to reject them, not the parser. So the bar here is simple:
//! every fixture parses with zero syntax errors, and the resulting tree is lossless.

use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/alpha-language-fixtures")
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

#[test]
fn every_fixture_parses_with_zero_syntax_errors() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (bundled fixtures missing)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    let mut failures = Vec::new();

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let parse = alpha_syntax::parse(&src);

        // Sanity: the tree must be lossless regardless of pass/fail.
        assert_eq!(
            parse.syntax_node().text().to_string(),
            src,
            "tree is not lossless for {path:?}"
        );

        if !parse.errors.is_empty() {
            failures.push((path.clone(), parse.errors));
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} fixture(s) had unexpected syntax errors:\n{:#?}",
        failures.len(),
        files.len(),
        failures
    );
}
