"""Tests for the `%%alpha`/`%%schedule` IPython cell magics (§5.2, §10.2).

Drives a real, in-process IPython shell (`IPython.testing.globalipapp`) rather than mocking magic
dispatch — the magics' whole job is to sit on top of IPython's own `run_cell_magic` machinery, so
that's the boundary worth testing against.
"""

import pytest

IPython = pytest.importorskip("IPython")
import IPython.testing.globalipapp as globalipapp  # noqa: E402

# `globalipapp.get_ipython` is reassigned in place (module attribute rebound to the real shell's
# bound method) the first time it actually starts the shell — `start_ipython()` is a run-once
# guard that returns `None` on every call after the first. Calling through the module attribute
# each time (rather than `from ... import get_ipython`, which would freeze a reference to the
# original stale wrapper) is what makes every test after the first one in a session get the real
# shell instead of `None`.
def get_ipython():
    return globalipapp.get_ipython()

PREFIX_SCAN_CELL = (
    "affine PrefixScan [N]->{:N>0}\n"
    "    inputs  X: [N]\n"
    "    outputs Y: [N]\n"
    "    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);\n"
    ".\n"
)

PREFIX_SCAN_SCHEDULE_CELL = "{ Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, j]; }\n"


@pytest.fixture
def ip():
    shell = get_ipython()
    shell.run_cell("import alpha")
    # `alpha.magics` self-registers via a module-level `get_ipython()` call, which is a no-op if
    # `alpha` was already imported (by an earlier test module, e.g. test_alpha.py) before this
    # fixture's shell existed — Python only runs a module's top level once. That's a pytest
    # cross-test artifact, not a product bug: a real notebook kernel always has its shell running
    # before any user `import` executes. Register explicitly so these tests don't depend on
    # cross-test import order.
    from alpha.magics import AlphaMagics

    shell.register_magics(AlphaMagics)
    yield shell
    for name in ("sys", "norm", "sched", "code", "bad", "broken"):
        shell.user_ns.pop(name, None)


def test_percent_alpha_binds_a_system(ip):
    result = ip.run_cell_magic("alpha", "sys", PREFIX_SCAN_CELL)
    assert result is None or not getattr(result, "error_in_exec", None)
    import alpha

    assert isinstance(ip.user_ns["sys"], alpha.System)


def test_percent_alpha_with_bad_source_does_not_bind(ip):
    result = ip.run_cell(f'%%alpha broken\nthis is not alpha source\n')
    assert result.error_in_exec is not None
    assert "broken" not in ip.user_ns


def test_percent_alpha_requires_a_variable_name(ip):
    result = ip.run_cell(f"%%alpha\n{PREFIX_SCAN_CELL}")
    assert isinstance(result.error_in_exec, ValueError)


def test_percent_schedule_binds_a_scheduled_system(ip):
    ip.run_cell_magic("alpha", "sys", PREFIX_SCAN_CELL)
    ip.run_cell("norm = alpha.normalize(sys)")
    result = ip.run_cell(f"%%schedule sched norm\n{PREFIX_SCAN_SCHEDULE_CELL}")
    assert result.error_in_exec is None
    import alpha

    assert isinstance(ip.user_ns["sched"], alpha.ScheduledSystem)


def test_percent_schedule_against_wrong_type_raises_type_error(ip):
    ip.run_cell_magic("alpha", "sys", PREFIX_SCAN_CELL)
    result = ip.run_cell(f"%%schedule bad sys\n{PREFIX_SCAN_SCHEDULE_CELL}")
    assert isinstance(result.error_in_exec, TypeError)


def test_percent_schedule_against_undefined_name_raises_name_error(ip):
    result = ip.run_cell(f"%%schedule bad nope\n{PREFIX_SCAN_SCHEDULE_CELL}")
    assert isinstance(result.error_in_exec, NameError)


def test_percent_schedule_with_illegal_schedule_raises_schedule_error(ip):
    ip.run_cell_magic("alpha", "sys", PREFIX_SCAN_CELL)
    ip.run_cell("norm = alpha.normalize(sys)")
    result = ip.run_cell("%%schedule bad norm\n{}\n")
    import alpha

    assert isinstance(result.error_in_exec, alpha.ScheduleError)
    assert "bad" not in ip.user_ns


def test_full_pipeline_through_magics_generates_c(ip):
    ip.run_cell_magic("alpha", "sys", PREFIX_SCAN_CELL)
    ip.run_cell("norm = alpha.normalize(sys)")
    ip.run_cell(f"%%schedule sched norm\n{PREFIX_SCAN_SCHEDULE_CELL}")
    result = ip.run_cell("code = alpha.generate(sched)")
    assert result.error_in_exec is None
    assert "#include" in ip.user_ns["code"]
