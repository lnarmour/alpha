"""End-to-end tests for the alpha-py bindings (docs/scheduled-codegen-design.md §10.1).

Exercises the same read -> normalize -> schedule -> generate pipeline a notebook user
would drive interactively, plus the two error paths the binding is responsible for
surfacing as proper Python exceptions rather than Rust panics or opaque diagnostics.
"""

import pytest

import alpha

PREFIX_SCAN = """affine PrefixScan [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
."""

PREFIX_SCAN_SCHEDULE = "{ Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, j]; }"


def test_parse_returns_a_system():
    sys = alpha.parse(PREFIX_SCAN)
    assert isinstance(sys, alpha.System)
    assert "Y[" in repr(sys)


def test_normalize_splits_the_top_level_reduce():
    sys = alpha.parse(PREFIX_SCAN)
    norm = alpha.normalize(sys)
    assert isinstance(norm, alpha.NormalizedSystem)
    text = repr(norm)
    assert "Y__init" in text
    assert "Y__reduce" in text


def test_normalize_does_not_mutate_its_input():
    sys = alpha.parse(PREFIX_SCAN)
    before = repr(sys)
    alpha.normalize(sys)
    assert repr(sys) == before


def test_schedule_accepts_a_valid_target_mapping():
    norm = alpha.normalize(alpha.parse(PREFIX_SCAN))
    sched = norm.schedule(PREFIX_SCAN_SCHEDULE)
    assert isinstance(sched, alpha.ScheduledSystem)


def test_generate_on_scheduled_system_produces_c_source():
    norm = alpha.normalize(alpha.parse(PREFIX_SCAN))
    sched = norm.schedule(PREFIX_SCAN_SCHEDULE)
    code = alpha.generate(sched)
    assert "#include" in code
    assert "Y" in code


def test_generate_on_identity_default_schedule_raises_schedule_error():
    norm = alpha.normalize(alpha.parse(PREFIX_SCAN))
    with pytest.raises(alpha.ScheduleError):
        alpha.generate(norm)


def test_generate_on_bare_system_raises_type_error():
    sys = alpha.parse(PREFIX_SCAN)
    with pytest.raises(TypeError):
        alpha.generate(sys)


def test_schedule_with_invalid_text_raises_schedule_error():
    norm = alpha.normalize(alpha.parse(PREFIX_SCAN))
    with pytest.raises(alpha.ScheduleError):
        norm.schedule("not a valid union map")


def test_schedule_error_message_carries_the_diagnostic():
    norm = alpha.normalize(alpha.parse(PREFIX_SCAN))
    with pytest.raises(alpha.ScheduleError, match="strictly before"):
        alpha.generate(norm)


def test_parse_with_syntax_error_raises_value_error():
    with pytest.raises(ValueError):
        alpha.parse("this is not alpha source")
