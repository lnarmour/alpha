# alpha-transform

`Normalize` and `NormalizeReduction` — the only two AST-to-AST transformation passes the
demand-driven codegen path depends on, built over [`alpha-model`](../alpha-model)'s resolved
domains.

The rest of the source project's transformation/scheduling family (tiling, memory-mapping,
reduction-simplification search, ...) is explicitly out of scope for this port.

## Modules

- **`ir`**: a new, owned, mutable "resolved AST" (`System`/`SystemBody`/`Equation`/`Expr`) —
  deliberately *not* built on `alpha_syntax::ast`'s rowan CST. `Normalize` is a term-rewriting pass
  that needs to replace nodes and recompute attached domains as it goes; rowan's tree is a
  persistent, structurally-shared structure meant for the opposite property (cheap, lossless
  *parse-time* edits). `Expr` carries its own `expression_domain: Set` and
  `context_domain: Option<Set>` directly as fields, no side-table.
- **`lower`**: builds an `ir::System` from an analyzed `ast::System` (via
  `alpha_model::domain::Resolver::analyze_system`) by *transcribing*, never re-deriving — every
  isl object a node needs comes from the same `Resolver` methods `alpha-model`'s phases 3–4 already
  used, so lowering can never disagree with analysis about a node's function/context. Equations
  that don't lower (a `ConvolutionExpression`'s own domain, or a fuzzy feature) are skipped, each
  contributing one diagnostic; the rest of the system still lowers.
- **`normalize`**: the ~25-rule `Normalize` port. Structural rewriting recomputes a rewritten
  node's `expression_domain` immediately; `context_domain` is recomputed in a separate top-down
  pass (`refresh_context`) run between rounds of structural rewriting (up to `MAX_ROUNDS = 8`),
  since a couple of rules need it fresh.
- **`normalize_reduction`**: `NormalizeReduction` — extracts every *top-level* `Reduce` in a
  `StandardEquation` into a fresh local + equation (skips `UseEquation`s and nested reductions,
  matching the source exactly). Run this **before** `Normalize` in the real pipeline — a
  `Dependence` directly wrapping a bare `Reduce` only reaches normal form once the reduction has
  somewhere else to live.

## Status

Done for scope. `normalize_fixtures` validates all 428 equations across every fixture that lowers
(from the bundled `tests/alpha-language-fixtures/` 82-file corpus), asserting every one reaches the source
system's documented normal form.

## Hardest-won bugs (read before extending `normalize.rs`)

Both silent-wrong-answer bugs, not compile errors:

1. Every "no rule matched, put this node back unchanged" fallback reconstructs via
   `expr_from_kind`, which (correctly, for a genuinely *new* node) always sets
   `context_domain: None`. Applied to an *unchanged* node, this silently wipes context on every
   node in the tree on every structural pass. Fixed by restoring the *original* node's context
   domain when `try_rewrite` reports "unchanged" — a node's own top-level shape not having changed
   means its context is still valid regardless of what its children's reconstruction did.
2. Relatedly, several "no rule matched" fallbacks used to call `expr_from_kind` on a peeled-off
   operand — which intentionally panics on a leaf (`Variable`/`Bool`/`Int`/`Real`), since a leaf's
   domain can't be recomputed bottom-up from children it doesn't have. But a "no rule fired"
   operand can absolutely *be* a leaf. Every such site now reconstructs via
   `Expr::new(other, saved_domain, saved_context)` using the operand's own already-correct fields,
   captured before the match moved `kind` out.

See `docs/progress.md` in the workspace root for further detail and the fixture programs that exposed each
bug.
