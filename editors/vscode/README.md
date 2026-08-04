# Alpha Language

Language support for **Alpha**, a functional, equational language for describing array
computations used in [polyhedral compilation](https://en.wikipedia.org/wiki/Polytope_model) —
a technique compilers use to analyze and transform loop-heavy numerical code (think dense linear
algebra, stencils, and other array-based kernels) for optimization and parallelization. Alpha
programs express *what* a computation is, as systems of equations over polyhedral domains, rather
than *how* to loop over it; a compiler like [`alphac`](https://github.com/lnarmour/alpha) turns
that specification into ordinary C.

This extension provides editor support for `.alpha` files:

- **Syntax highlighting**, so Alpha source is readable at a glance.
- **Live diagnostics** — parse errors, unresolved names, domain and uniqueness violations, and
  other correctness checks — reported directly in the editor as you type, powered by the same
  analysis engine used by the `alphac` compiler itself.

No separate language server or external process is required; the checks run in-process, so
feedback appears immediately after opening, editing, or saving a file.

## Learn more

For the language itself, the compiler, and the project's source, see the
[alpha-rs repository](https://github.com/lnarmour/alpha).
