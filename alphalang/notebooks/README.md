# Notebook fixtures

`prefix_sum.ipynb` is `docs/scheduled-codegen-design.md` §5.2's own worked example, executed for
real and checked in with its real outputs — a regression fixture, not just documentation (§5.3,
§12 step 8).

Run it:

```
pytest --nbval alphalang/notebooks/prefix_sum.ipynb
```

`--nbval` re-executes every code cell against a live kernel and diffs the result against the
outputs already saved in the file; plain `pytest` (no `--nbval`) does not collect `.ipynb` files at
all, so this never runs as part of the ordinary `alphalang/tests/` suite. It needs the `python3`
Jupyter kernel registered against this repo's `.venv` (already the case if you've run `jupyter
kernelspec list` and see `python3 -> .../alpha-rs/.venv/share/jupyter/kernels/python3`; if not,
`python -m ipykernel install --user` from an activated `.venv`).

Generated C and `__repr__` output here are fully deterministic (no timestamps, no addresses) — a
diff on re-run means an actual behavior change in `alphalang`/`alpha-codegen`, not fixture flakiness.

To regenerate after an intentional change, re-execute and overwrite in place:

```
jupyter nbconvert --to notebook --execute --inplace alphalang/notebooks/prefix_sum.ipynb
```

then review the diff like any other fixture update — every output line changing is exactly what
this is here to catch.
