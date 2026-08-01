# barvinok-sys

Raw FFI bindings to [libbarvinok](https://barvinok.sourceforge.io/) (Ehrhart/cardinality
counting), mirroring [`isl-sys`](../isl-sys)'s role for isl.

## Status: stub

Not yet implemented — this crate has no bindings and no dependencies yet. It's intentionally left
empty until `alpha-codegen`'s cardinality-counting milestone (Barvinok-based `malloc` sizing for
non-rectangular domains) actually needs it — see [`alpha-codegen`'s README](../alpha-codegen) for
the isl-only bounding-box fallback used in the meantime. Deferred rather than built speculatively,
since the isl-only fallback covers every real fixture in the corpus today.

## Why a separate crate

**Barvinok is GPL** (unlike isl, which is MIT — even though barvinok vendors isl internally since
version 0.30, the standalone isl project itself stays MIT). Splitting the bindings into their own
crate family, parallel to but independent from `isl-sys`/`isl`, is a **license boundary**, not
just cleanliness: anything that links `barvinok` transitively takes on GPL obligations on
distribution, while anything that only links `isl` does not.

Verify the exact GPL version against barvinok's own `COPYING` file before this crate is actually
implemented or published — the `Cargo.toml` here currently states `GPL-2.0-or-later` as a
placeholder and deliberately does **not** inherit the workspace's MIT default.

## Who may depend on this

Nothing in the default build of `alphac` or the VS Code extension may depend on this crate, now
or later — only `alpha-codegen`'s optional, off-by-default `barvinok` Cargo feature (which in turn
depends on the safe wrapper, [`barvinok`](../barvinok)) may.

## License

GPL-2.0-or-later (placeholder, pending verification against barvinok's own `COPYING` — see
above). Opt-in only.
