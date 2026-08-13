# alphalang

A PyO3 binding crate exposing `alpha-syntax`/`alpha-model`/`alpha-transform`/`alpha-codegen`'s
`ScheduledC` pipeline (`docs/scheduled-codegen-design.md`) to Python, plus an IPython cell-magic
front end for driving it interactively from a Jupyter notebook. The Python package name is
**`alphalang`**; the compiled extension module is `alphalang._alpha` (crate name `_alpha`, per
`Cargo.toml` — PyO3's `extension-module` feature, so it links against no libpython at build time
and is resolved by the interpreter at import time).

`alphac`'s `WriteC` CLI path is untouched by any of this — `ScheduledC` has no CLI of its own and is
driven entirely through this crate.

## Layout

- **`src/lib.rs`**: the whole binding. Three `#[pyclass(frozen, unsendable)]` types —
  `System` → `NormalizedSystem` → `ScheduledSystem` — each an immutable wrapper around a *cloned*
  `alpha_transform::ir::System` (cloning it is a cheap isl-refcount bump, not a real copy; see
  `ir::System`'s own doc comment for why `Clone` was added there). Being three distinct Python
  types, not one mutable class with a state enum, is what turns "you must normalize before you can
  schedule" into a `TypeError` the binding raises before any Rust code runs, rather than a runtime
  diagnostic buried in the pipeline. `unsendable` on all three: isl's raw C pointers aren't `Sync`,
  and isl itself isn't thread-safe — `unsendable` is the honest opt-out (restricted to the thread
  that created the value), not a workaround.
- **`python/alphalang/__init__.py`**: re-exports the compiled extension's types/functions; imports
  `alphalang.magics` in a `try`/`except ImportError` (a no-op outside IPython).
- **`python/alphalang/magics.py`**: `AlphaLangMagics(Magics)` — the `%%alphalang <var>` and
  `%%schedule <var> <source-system-var>` IPython cell magics, self-registering on `import alphalang`.
- **`notebooks/prefix_sum.ipynb`**: a real, executed-and-checked-in worked example (also a
  regression fixture — see its own `notebooks/README.md`).

## Python API

| Function/method | Returns | Notes |
|---|---|---|
| `alphalang.read(path)` | `System` | parse + analyze + lower a `.alpha` file from disk |
| `alphalang.parse(source)` | `System` | same, from an inline string — what `%%alphalang` is sugar for |
| `alphalang.normalize(sys)` | `NormalizedSystem` | runs `normalize_reduction::apply` then `normalize::apply` (that order is required — see `alpha-transform`'s own README) against a clone; `sys` is untouched |
| `norm.schedule(text)` | `ScheduledSystem` | parses + validates (§6) + legality-checks (§7) a target mapping against a clone of `norm`; raises `alphalang.ScheduleError` and binds nothing on any failure |
| `alphalang.generate(system)` | `str` | `NormalizedSystem` or `ScheduledSystem`; a bare `NormalizedSystem` is sugar for `generate(norm.schedule(""))` (§6's identity-schedule default) |
| `alphalang.generate_wrapper(system)` | `str` | `NormalizedSystem` or `ScheduledSystem`; a `*_wrapper.c`-style test harness (issue #23) for the system's public entry point — allocates memory for every parameter, calls the generated function, and frees it. Scheduling never changes the entry point's own signature, so this accepts either stage identically, unlike `generate` |
| `repr(sys)` / `repr(norm)` / `repr(sched)` | `str` | the ISL union-map skeleton, no precondition — always safe to print |
| `alphalang.print(system)` | `str` | an indented debug tree dump — every node's own kind plus its `expression_domain`/`context_domain` (ported from alpha-language's `PrintAST`); accepts `System`, `NormalizedSystem`, or `ScheduledSystem` |
| `alphalang.show(system)` | `str` | reconstructs Alpha-like source syntax from the model, `f@X` point-free `Dependence` notation (ported from `Show.xtend`); same three accepted types |
| `alphalang.ashow(system)` | `str` | like `show`, but array-index notation (`X[f]`) for a `Dependence` over a `Variable`/constant, and explicit ambient index names on each equation (ported from `AShow.xtend`) |

`alphalang.ScheduleError` is defined via `pyo3::create_exception!`, not a hand-rolled
`#[pyclass(extends = PyException)]` unit struct — the latter compiles and even raises without
error, but has no usable `__new__`, so the *first time real Python code actually catches one*, PyO3
fails with `TypeError: No constructor defined for ScheduleError` instead of yielding the exception
instance. Only surfaced by testing the catch side from real Python; invisible from `cargo test` or
from the raise side alone.

## Hello world

```
uv sync && source .venv/bin/activate   # from the repo root — builds and installs alpha too
```

```python
import alphalang

sys = alphalang.parse("""
affine PrefixSum [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
.
""")
norm = alphalang.normalize(sys)
sched = norm.schedule(
    "{ Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, j]; }"
)
print(alphalang.generate(sched))
```

Or interactively in Jupyter, using the cell magics instead of `parse`/`schedule` strings directly —
see `notebooks/prefix_sum.ipynb` for the full worked version.

**After changing Rust code**, `uv sync` alone won't pick it up — it only rebuilds a workspace
member when it decides a reinstall is warranted, and mtime-only/no-op edits don't trigger that.
Force it explicitly:

```
uv sync --reinstall-package alphalang
```

(`maturin develop` from inside `alphalang/` also still works, and is faster for a tight edit/test
loop since it skips uv's dependency resolution step — but isn't required any more.) Plain
`cargo build -p alphalang` fails to link either way — expected for a PyO3 `extension-module` crate
(Python symbols are resolved by the interpreter at runtime, not at link time).

## Testing

```
pytest alphalang/tests/                              # 26 tests: plain API + magics
pytest --nbval alphalang/notebooks/prefix_sum.ipynb  # 8 more: the notebook fixture
```

`tests/test_magics.py` drives a real in-process IPython shell
(`IPython.testing.globalipapp`) rather than mocking magic dispatch. One non-obvious thing if you
extend it: `globalipapp.get_ipython` is reassigned in place — rebound to the real shell's own bound
method — the first time it actually starts a shell; `from IPython.testing.globalipapp import
get_ipython` freezes a reference to the original wrapper, which returns `None` on every call after
the first (`start_ipython()`'s "run once" guard). Call it through the module attribute
(`globalipapp.get_ipython()`) instead, every time, not via a top-level `from`-import.

## Status

Done for scope (`docs/scheduled-codegen-design.md` §12, phasing step 7 — see it for the full list
of findings). JupyterLab syntax highlighting for `%%alphalang`/`%%schedule` cells was prototyped and
then deliberately dropped, not shipped: JupyterLab 4 has no extension point mapping a cell-magic
prefix to a highlighted language at all, so it would only ever have covered standalone `.alpha`
files, not notebook cells — not worth a JS/TypeScript toolchain for that. Nothing functional
depends on it; cells stay plain-text.
