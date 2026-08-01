# barvinok

Safe Rust wrapper over [`barvinok-sys`](../barvinok-sys), mirroring [`isl`](../isl)'s role for
`isl-sys`.

## Status: stub

Not yet implemented — depends on `barvinok-sys` (itself unimplemented) and `isl`, but has no
wrapper types or operations of its own yet. This lands alongside `alpha-codegen`'s Ehrhart/
cardinality-counting milestone, deferred until a real fixture needs exact (non-bounding-box)
`malloc` sizing — see [`alpha-codegen`'s README](../alpha-codegen).

## Why this exists as its own crate

Same reasoning as `barvinok-sys`: **Barvinok is GPL**, isl is not, so the GPL surface is isolated
to this crate pair rather than infecting `isl`/`isl-sys`. Only `alpha-codegen`'s optional
`barvinok` Cargo feature may depend on this crate — the default `alphac` build and the VS Code
native addon must never pull it in.

## License

GPL-2.0-or-later. Opt-in only.
