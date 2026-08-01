//! The parser must never panic, no matter how malformed the input — that's the whole point of
//! a resilient, lossless CST for editor use (live diagnostics on invalid/mid-edit code). This
//! fuzzes truncated prefixes of every real fixture, plus a few hand-picked garbage inputs, and
//! checks only for (a) no panic and (b) a lossless tree.

use std::path::{Path, PathBuf};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../alpha-language/tests")
}

fn all_alpha_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            all_alpha_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "alpha") {
            out.push(path);
        }
    }
}

fn check_no_panic(label: &str, src: &str) {
    let parse = std::panic::catch_unwind(|| alpha_syntax::parse(src))
        .unwrap_or_else(|_| panic!("parser panicked on {label}"));
    assert_eq!(
        parse.syntax_node().text().to_string(),
        src,
        "tree not lossless for {label}"
    );
}

#[test]
fn hand_picked_garbage_never_panics() {
    for (label, src) in [
        ("empty", ""),
        ("just whitespace", "   \n\t  "),
        ("just a comment", "/* unterminated"),
        ("just a keyword", "affine"),
        ("lone brace", "{"),
        ("lone close brace", "}"),
        ("unmatched parens", "((((("),
        ("garbage symbols", "@@@ ??? $$$ !!!"),
        ("truncated system", "affine Foo [N"),
        ("truncated domain", "affine Foo [N]->{:"),
        ("truncated equation", "affine Foo [N]->{:} let X ="),
        ("stray dot", "."),
        ("stray semicolon soup", ";;;;;;"),
        ("nested unterminated braces", "affine F []->{: {{{{"),
        ("reduce with nothing", "affine F []->{:} let X = reduce("),
        ("dependence dangling at", "affine F []->{:} let X = (i->i)@"),
    ] {
        check_no_panic(label, src);
    }
}

#[test]
fn truncated_prefixes_of_every_fixture_never_panic() {
    let root = fixtures_root();
    if !root.exists() {
        eprintln!("skipping: {root:?} not found (expected sibling alpha-language checkout)");
        return;
    }
    let mut files = Vec::new();
    all_alpha_files(&root, &mut files);
    assert!(!files.is_empty(), "found no .alpha fixtures under {root:?}");

    for path in &files {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        // Every 17th byte offset (arbitrary stride, just to keep the test fast while still
        // covering a wide variety of "cut off mid-token/mid-construct" positions across all 82
        // files) — truncated at a char boundary.
        let mut i = 0;
        while i < src.len() {
            let mut end = i;
            while !src.is_char_boundary(end) {
                end += 1;
            }
            check_no_panic(&format!("{}:{}", path.display(), end), &src[..end]);
            i += 17;
        }
    }
}
