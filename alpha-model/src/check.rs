//! `check_source` — the single "check a whole raw source file" entry point IDE-style callers
//! want (in particular, the VS Code extension's native binding — see `docs/design.md`): unlike
//! [`crate::analyze_root`], which takes an already-parsed [`alpha_syntax::ast::Root`] and assumes
//! the caller already decided what to do about syntax errors (mirroring `alphac`'s own
//! `parse → bail-on-syntax-error → analyze` split), this takes raw text and always returns one
//! flat, ordered diagnostic stream — syntax errors as [`Diagnostic::Syntax`] (the variant exists
//! specifically for this: "carried through unchanged so callers have one combined diagnostic
//! stream instead of two", per its own doc comment), or every system's semantic diagnostics
//! otherwise.
//!
//! Mirrors `alphac`'s own short-circuit exactly: semantic analysis only runs if the source parses
//! clean. Running phases 1–6 against the parser's resilient error-recovery output (rather than a
//! genuinely well-formed tree) is untested territory — no fixture in the corpus exercises it, and
//! the six phases' own panics-vs-`Result` discipline was only ever validated against trees free of
//! `ERROR`/missing nodes.

use crate::{Diagnostic, analyze_root};
use alpha_syntax::ast;

/// Parses `source` and returns one diagnostic per problem found, each paired with the name of the
/// system it belongs to (`None` for a syntax error, which precedes any system, or for a
/// whole-program diagnostic with nowhere else to attach — see [`analyze_root`]'s own doc comment
/// on where [`crate::uniqueness::check_program_uniqueness`]'s diagnostics land).
pub fn check_source(source: &str) -> Vec<(Option<String>, Diagnostic)> {
    let parse = alpha_syntax::parse(source);
    if !parse.errors.is_empty() {
        return parse
            .errors
            .into_iter()
            .map(|e| (None, Diagnostic::Syntax(e)))
            .collect();
    }

    let tree: ast::Root = parse.tree();
    let ctx = isl::Context::new();
    analyze_root(&ctx, &tree)
        .into_iter()
        .flat_map(|(name, diags)| {
            let name = if name.is_empty() { None } else { Some(name) };
            diags
                .into_iter()
                .map(move |d| (name.clone(), d))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::check_source;
    use crate::Diagnostic;
    use std::path::{Path, PathBuf};

    fn fixtures_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../alpha-language/tests")
    }

    #[test]
    fn syntactically_broken_source_reports_only_syntax_diagnostics() {
        // Missing the closing `.` terminator and a dangling `let` — never valid Alpha regardless
        // of the sibling fixture checkout being present.
        let broken = "affine broken [N]->{:N>0} outputs A:[N]; let A[i] =";
        let diags = check_source(broken);
        assert!(!diags.is_empty(), "expected at least one syntax diagnostic");
        assert!(
            diags
                .iter()
                .all(|(system, d)| system.is_none() && matches!(d, Diagnostic::Syntax(_))),
            "expected only Diagnostic::Syntax entries with no system name: {diags:#?}"
        );
    }

    #[test]
    fn known_valid_fixture_has_zero_diagnostics() {
        let path = fixtures_root().join("alpha.model.tests/resources/src-valid/basic/FFT.alpha");
        if !path.exists() {
            eprintln!("skipping: {path:?} not found (expected sibling alpha-language checkout)");
            return;
        }
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let diags = check_source(&src);
        assert!(diags.is_empty(), "expected zero diagnostics: {diags:#?}");
    }

    #[test]
    fn known_invalid_fixture_reports_expected_diagnostic() {
        let path = fixtures_root()
            .join("alpha.model.tests/resources/src-invalid/syntax-tests/systemBody1.alpha");
        if !path.exists() {
            eprintln!("skipping: {path:?} not found (expected sibling alpha-language checkout)");
            return;
        }
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
        let diags = check_source(&src);
        assert!(
            diags.iter().any(|(system, d)| system.as_deref() == Some("systemBody1b")
                && matches!(d, Diagnostic::EmptySystemBody { .. })),
            "expected EmptySystemBody for systemBody1b: {diags:#?}"
        );
        assert!(
            diags.iter().any(|(system, d)| system.as_deref() == Some("systemBody1c")
                && matches!(d, Diagnostic::IncompleteSystem { .. })),
            "expected IncompleteSystem for systemBody1c: {diags:#?}"
        );
    }
}
