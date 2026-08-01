# alpha-syntax

Lexer, lossless CST, resilient parser, and typed AST layer for the Alpha language — the front end
everything else in this workspace builds on.

This follows the same architecture rust-analyzer, rustc's own `rustc_parse` lineage, and most
modern "IDE-first" language tools use. A hand-written parser (rather than a generator like
`pest`/`lalrpop`) is what makes two Alpha-specific irregularities tractable: the opaque
"capture until matching brace" domain/relation/function literals (see below), and the `over`/
`with` optional-clause soup in `UseEquation` — a generator fights you on both.

## Pipeline

```
text --logos--> tokens --hand-written recursive-descent/Pratt parser--> rowan CST --typed layer--> ast::
```

- **`lexer`/`token_kind`**: a `logos`-derived lexer for Alpha's token set.
- **`parser`** (`parser::items`, `parser::expr`, `parser::calculator`): a hand-written
  recursive-descent parser with a Pratt parser for the expression precedence chain (`if →
  restrict → or → and → relational → additive → multiplicative → minmax → unary`). Hand-written
  beats a parser-generator here for two Alpha-specific irregularities generators handle poorly:
  opaque "capture until matching brace" domain/relation/function literals (the text inside `{...}`
  isn't parsed by the grammar at all — see below), and the `over`/`with` optional-clause soup in
  `UseEquation`.
- **`syntax_kind`**: the `rowan` `SyntaxNode`/`SyntaxKind` plumbing — a **lossless, error-tolerant
  concrete syntax tree**. Every token, space, and comment is preserved (concatenating every leaf
  token reproduces the source exactly); parse errors don't abort the parse, they attach to a
  partial tree and the parser recovers at the next statement boundary. This is what makes the tree
  usable for editor tooling (hover, outline, partial diagnostics) mid-edit with invalid syntax.
- **`ast`**: a thin typed accessor layer over the CST (`ast::Root`, `ast::System`, `ast::Expr`,
  ...), in the spirit of rust-analyzer's `syntax::ast` — `Copy`-cheap wrappers that know how to
  find their own meaningful children. No semantic information lives here (name resolution,
  resolved ISL domains) — that's `alpha-model`'s job; this crate only knows about syntax.

## A load-bearing quirk of the grammar

The text inside `{ ... }` domain/relation/function literals is **not parsed by this grammar at
all** — it's captured as an opaque token-soup substring and handed to isl's own string parser
later, during semantic analysis (`alpha-model`). So "the domain sub-grammar" is really isl's
Presburger-arithmetic syntax, not something this crate defines. The parser's job here is just to
find the matching brace and capture the span.

## Status

Done: lexer, rowan CST, resilient recursive-descent/Pratt parser, and the typed `ast::` layer are
all implemented and fixture-tested against the full 82-file `.alpha` corpus in the sibling
`alpha-language` repo (see the workspace root README's "Fixture corpus" section):

- `lex_fixtures` — 82 files, 0 lex errors
- `parse_fixtures` — 82 files, 0 syntax errors
- `ast_fixtures` — 271 systems / 345 equations walked
- `resilience` — hand-picked garbage input plus every ~17-byte truncation of every fixture, zero
  panics
- plus unit tests in `parser`/`lexer`

## Non-obvious pitfalls (read before touching the parser)

1. **Trivia-attachment timing**: `start_node` must flush pending trivia *except* on the very first
   call (which opens `ROOT`); `finish_node` must *never* flush (trailing trivia belongs to
   whichever ancestor's next `bump()` picks it up). Getting this wrong doesn't break parsing —
   it silently corrupts node *text* (e.g. leading/trailing whitespace creeping into a node's
   `.text()`), which only shows up once something reads raw node text, as `alpha-model` does
   pervasively.
2. **Lexical errors must stay in the token stream** (as `ERROR` leaves), never dropped — otherwise
   a byte the lexer can't tokenize silently vanishes from the tree, breaking losslessness.
3. `ParamDomain::param_names()` / `RectangularDomain::index_names()` must stop at the right
   delimiter (`->` / `as`) — these nodes have no wrapper node around their `{...}` body or
   bound-list, so a naive "all IDENT children" collection also picks up identifiers used *inside*
   the constraint/bound text.

See `docs/progress.md` in the workspace root for more detail on these and other hard-won findings.
