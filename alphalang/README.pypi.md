# alphalang

Python bindings for [Alpha](https://github.com/lnarmour/alpha), a polyhedral compilation language
for expressing and transforming affine programs. `alphalang` exposes the Rust
parse → normalize → schedule → generate pipeline as a small, typed Python API, plus IPython cell
magics (`%%alphalang`, `%%schedule`) for driving it interactively from a Jupyter notebook.

## Install

```
pip install alphalang
```

The distribution is named `alphalang`, but the importable package is `alphalang`.

## Quick start

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

See the [project README](https://github.com/lnarmour/alpha/blob/main/alphalang/README.md) for the
full API, a worked Jupyter notebook example, and source.
