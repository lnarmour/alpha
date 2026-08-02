//! Lexer conformance check against the real `.alpha` corpus from the sibling `alpha-language`
//! (Java/Xtext) repo. At this milestone we only assert zero *lexical* errors (every character
//! turns into some token) — these fixtures include both `src-valid` and `src-invalid` files,
//! but even the `src-invalid` ones are almost all *semantically* or *syntactically* invalid at
//! the parser level, not lexically invalid, so this is still a meaningful conformance bar ahead
//! of the parser milestone.

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
fn every_fixture_tokenizes_without_lexical_errors() {
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
        let (_tokens, errors) = alpha_syntax::tokenize(&src);
        if !errors.is_empty() {
            failures.push((path.clone(), errors));
        }
    }

    assert!(
        failures.is_empty(),
        "{}/{} fixtures had lexical errors:\n{:#?}",
        failures.len(),
        files.len(),
        failures
    );
}
