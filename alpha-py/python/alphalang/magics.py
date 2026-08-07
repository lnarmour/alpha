"""`%%alpha`/`%%schedule` IPython cell magics (docs/scheduled-codegen-design.md §5.2, §10.2).

Imported (and self-registered against the running kernel) by ``alphalang/__init__.py``; importing
this module outside IPython is a no-op beyond the class definition — nothing runs until IPython's
own magic dispatch calls into it. Not meant to be imported directly by user code.
"""

from IPython.core.getipython import get_ipython
from IPython.core.magic import Magics, cell_magic, magics_class

from ._alpha import NormalizedSystem, parse


@magics_class
class AlphaMagics(Magics):
    """Registers ``%%alpha`` and ``%%schedule`` — see §5.2 for the full worked example."""

    @cell_magic
    def alpha(self, line, cell):
        """``%%alpha <var>`` — parses the cell body and binds an `alphalang.System` to `<var>`."""
        var = line.strip()
        if not var:
            raise ValueError("%%alpha requires a variable name: %%alpha <var>")
        system = parse(cell)
        self.shell.user_ns[var] = system

    @cell_magic
    def schedule(self, line, cell):
        """``%%schedule <var> <source-system-var>`` — binds an `alphalang.ScheduledSystem` to
        `<var>`.

        `<source-system-var>` must already name an `alphalang.NormalizedSystem` in the notebook
        namespace (§5.1's precondition) — a `TypeError`/`NameError` here, before the cell body is
        even parsed, matches every other type-gated operation in this API.
        """
        parts = line.split()
        if len(parts) != 2:
            raise ValueError(
                "%%schedule requires two names: %%schedule <var> <source-system-var>"
            )
        var, source_name = parts
        user_ns = self.shell.user_ns
        if source_name not in user_ns:
            raise NameError(f"name {source_name!r} is not defined")
        source = user_ns[source_name]
        if not isinstance(source, NormalizedSystem):
            raise TypeError(
                f"%%schedule's source system {source_name!r} must be a NormalizedSystem, "
                f"got {type(source).__name__} instead — run alphalang.normalize() first (§5.1)"
            )
        scheduled = source.schedule(cell)
        user_ns[var] = scheduled


_ip = get_ipython()
if _ip is not None:
    _ip.register_magics(AlphaMagics)
