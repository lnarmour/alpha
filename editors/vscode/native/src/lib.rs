//! Thin napi-rs shim over [`alpha_model::check_source`] — the VS Code extension's only call into
//! Rust. Deliberately does no analysis of its own: every domain/context-domain/completeness/
//! uniqueness rule lives in `alpha-model`, and every parse/syntax-error rule lives in
//! `alpha-syntax`; this crate only translates the resulting `Vec<(Option<String>, Diagnostic)>`
//! into a napi-friendly shape.
//!
//! Exposed as a plain **synchronous** `#[napi]` function, not an async/threadsafe one:
//! `isl::Context` (which `check_source` constructs internally, fresh per call) is an `Rc` wrapper
//! around a raw `isl_ctx` pointer with no `Send`/`Sync` impl, so it cannot cross threads. Running
//! on the calling (JS) thread and blocking is the only option without a much larger redesign —
//! acceptable given `.alpha` files are small and analysis is fast.

#![deny(clippy::all)]

use napi_derive::napi;

#[napi(object)]
pub struct JsDiagnostic {
    pub message: String,
    pub start: u32,
    pub end: u32,
    pub system: Option<String>,
}

#[napi]
pub fn check_alpha_source(source: String) -> Vec<JsDiagnostic> {
    alpha_model::check_source(&source)
        .into_iter()
        .map(|(system, diagnostic)| {
            let (start, end) = diagnostic.range();
            JsDiagnostic {
                message: diagnostic.to_string(),
                start,
                end,
                system,
            }
        })
        .collect()
}
