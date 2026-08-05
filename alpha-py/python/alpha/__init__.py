"""Interactive Python front end for Alpha's scheduled codegen (docs/scheduled-codegen-design.md).

Five pipeline stages split across two mechanisms (§5.2): reading Alpha/target-mapping *source
text* happens through IPython cell magics (``%%alpha``, ``%%schedule`` — registered on import, see
``alpha.magics``); everything else — ``normalize``/``schedule``/``generate`` — is a plain, typed
Python function or method over immutable values, each cloning its input rather than mutating it.

``print``/``show``/``ashow`` are read-only renderings of a ``System``/``NormalizedSystem``/
``ScheduledSystem`` at any pipeline stage (ported from alpha-language's own
``PrintAST``/``Show``/``AShow``): ``print`` is an indented debug tree dump, ``show`` reconstructs
Alpha-like source syntax, ``ashow`` is ``show`` in array-index notation (``X[i+1,j]`` instead of
``show``'s point-free ``f@X``).

    >>> import alpha
    >>> sys = alpha.read("foo.alpha")
    >>> print(alpha.show(sys))
    >>> norm = alpha.normalize(sys)
    >>> sched = norm.schedule("{ ... }")
    >>> code = alpha.generate(sched)
"""

from ._alpha import (
    NormalizedSystem,
    ScheduledSystem,
    ScheduleError,
    System,
    ashow,
    generate,
    normalize,
    parse,
    print,
    read,
    show,
)

__all__ = [
    "System",
    "NormalizedSystem",
    "ScheduledSystem",
    "ScheduleError",
    "read",
    "parse",
    "normalize",
    "generate",
    "print",
    "show",
    "ashow",
]

# Registers the %%alpha/%%schedule cell magics (§5.2, §10.2) if running inside IPython — a no-op
# import-time side effect outside a notebook/IPython shell (phase 9; not yet implemented as of
# phase 8, see docs/scheduled-codegen-design.md §12 step 8).
try:
    from . import magics as _magics  # noqa: F401
except ImportError:
    pass
