"""End-to-end tests for the alphalang bindings (docs/scheduled-codegen-design.md §10.1).

Exercises the same read -> normalize -> schedule -> generate pipeline a notebook user
would drive interactively, plus the two error paths the binding is responsible for
surfacing as proper Python exceptions rather than Rust panics or opaque diagnostics.
"""

import pytest

import alphalang

PREFIX_SUM = """affine PrefixSum [N]->{:N>0}
    inputs  X: [N]
    outputs Y: [N]
    let Y[i] = reduce(+, [j], {:j<=i}: X[j]);
."""

PREFIX_SUM_SCHEDULE = "{ Y__init[i] -> [i, 0, 0]; Y__reduce[i,j] -> [i, 1, j]; }"


def test_parse_returns_a_system():
    sys = alphalang.parse(PREFIX_SUM)
    assert isinstance(sys, alphalang.System)
    assert "Y[" in repr(sys)


def test_normalize_splits_the_top_level_reduce():
    sys = alphalang.parse(PREFIX_SUM)
    norm = alphalang.normalize(sys)
    assert isinstance(norm, alphalang.NormalizedSystem)
    text = repr(norm)
    assert "Y__init" in text
    assert "Y__reduce" in text


def test_normalize_does_not_mutate_its_input():
    sys = alphalang.parse(PREFIX_SUM)
    before = repr(sys)
    alphalang.normalize(sys)
    assert repr(sys) == before


def test_schedule_accepts_a_valid_target_mapping():
    norm = alphalang.normalize(alphalang.parse(PREFIX_SUM))
    sched = norm.schedule(PREFIX_SUM_SCHEDULE)
    assert isinstance(sched, alphalang.ScheduledSystem)


def test_generate_on_scheduled_system_produces_c_source():
    norm = alphalang.normalize(alphalang.parse(PREFIX_SUM))
    sched = norm.schedule(PREFIX_SUM_SCHEDULE)
    code = alphalang.generate(sched)
    assert "#include" in code
    assert "Y" in code


def test_generate_on_identity_default_schedule_raises_schedule_error():
    norm = alphalang.normalize(alphalang.parse(PREFIX_SUM))
    with pytest.raises(alphalang.ScheduleError):
        alphalang.generate(norm)


def test_generate_on_bare_system_raises_type_error():
    sys = alphalang.parse(PREFIX_SUM)
    with pytest.raises(TypeError):
        alphalang.generate(sys)


def test_schedule_with_invalid_text_raises_schedule_error():
    norm = alphalang.normalize(alphalang.parse(PREFIX_SUM))
    with pytest.raises(alphalang.ScheduleError):
        norm.schedule("not a valid union map")


def test_schedule_error_message_carries_the_diagnostic():
    norm = alphalang.normalize(alphalang.parse(PREFIX_SUM))
    with pytest.raises(alphalang.ScheduleError, match="strictly before"):
        alphalang.generate(norm)


def test_parse_with_syntax_error_raises_value_error():
    with pytest.raises(ValueError):
        alphalang.parse("this is not alpha source")


def test_generate_wrapper_on_normalized_system_produces_c_harness():
    norm = alphalang.normalize(alphalang.parse(PREFIX_SUM))
    harness = alphalang.generate_wrapper(norm)
    assert "#include" in harness
    assert "int main(" in harness
    assert "PrefixSum(" in harness


def test_generate_wrapper_on_scheduled_system_produces_c_harness():
    norm = alphalang.normalize(alphalang.parse(PREFIX_SUM))
    sched = norm.schedule(PREFIX_SUM_SCHEDULE)
    harness = alphalang.generate_wrapper(sched)
    assert "#include" in harness
    assert "int main(" in harness
    assert "PrefixSum(" in harness


def test_generate_wrapper_on_bare_system_raises_type_error():
    sys = alphalang.parse(PREFIX_SUM)
    with pytest.raises(TypeError):
        alphalang.generate_wrapper(sys)


def test_print_dumps_the_tree_with_domains():
    sys = alphalang.parse(PREFIX_SUM)
    text = alphalang.print(sys)
    assert "PrefixSum" in text
    assert "Reduce" in text
    assert "exp=" in text
    assert "ctx=" in text


def test_show_reconstructs_alpha_like_source():
    sys = alphalang.parse(PREFIX_SUM)
    text = alphalang.show(sys)
    assert text.startswith("affine PrefixSum")
    assert "Y = reduce(+," in text
    assert text.rstrip().endswith(".")


def test_ashow_shows_ambient_index_names():
    sys = alphalang.parse(PREFIX_SUM)
    text = alphalang.ashow(sys)
    assert "Y[i] = reduce(+," in text
    assert "Y[i] =" not in alphalang.show(sys)


def test_print_show_ashow_accept_any_pipeline_stage():
    sys = alphalang.parse(PREFIX_SUM)
    norm = alphalang.normalize(sys)
    sched = norm.schedule(PREFIX_SUM_SCHEDULE)
    for renderer in (alphalang.print, alphalang.show, alphalang.ashow):
        for value in (sys, norm, sched):
            assert isinstance(renderer(value), str)


def test_print_on_non_system_raises_type_error():
    with pytest.raises(TypeError):
        alphalang.print(42)
