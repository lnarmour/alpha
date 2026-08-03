# Alpha(Z) Language

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

## Requirements

Diagnostics rely on the `isl` and `gmp` libraries being installed and discoverable on your
system:

- **macOS**: `brew install isl gmp`
- **Linux**: install `libisl`/`libgmp` via your distro's package manager

If these aren't found, syntax highlighting keeps working, but diagnostics are disabled and the
extension shows a notice explaining how to point it at a custom install location (via the
`alpha.nativeLibraryPaths` setting).

Windows is not currently supported.

## Learn more

For the language itself, the compiler, and the project's source, see the
[alpha-rs repository](https://github.com/lnarmour/alpha).
