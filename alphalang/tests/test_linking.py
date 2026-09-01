import importlib.util
import sys

import pytest

import alphalang


GUPPY_AVAILABLE = sys.version_info >= (3, 12) and importlib.util.find_spec("guppylang")
requires_guppy = pytest.mark.skipif(
    not GUPPY_AVAILABLE, reason="Guppy requires Python 3.12"
)

ALPHA_BOOL_ARRAY = """affine kernel [N] -> {:N>0}
outputs M : {[i] : 0 <= i < N} of bool;
locals linear Q : {[i] : 0 <= i < N} of qubit;
let
    with [i] : (Q[i]) = qalloc();
    with [i] : (M[i]) = measure(Q[i]);
.
"""

ALPHA_BOOL_ARRAY_SCHEDULE = """[N] -> {
Q__call0[i] -> [0,i]; M__call0[i] -> [1,i]
}"""


def alpha_bool_array_hugr() -> str:
    normalized = alphalang.normalize(alphalang.parse(ALPHA_BOOL_ARRAY))
    scheduled = normalized.schedule(ALPHA_BOOL_ARRAY_SCHEDULE)
    return alphalang.generate_hugr(scheduled, {"N": 4})


def named_functions(module, symbol):
    from hugr.ops import FuncDecl, FuncDefn

    return [
        node
        for node in module.children(module.module_root)
        if isinstance(module[node].op, (FuncDecl, FuncDefn))
        and module[node].op.f_name == symbol
    ]


def count_calls_to(module, target):
    from hugr.ops import Call

    count = 0
    for node, node_data in module.items():
        if not isinstance(node_data.op, Call):
            continue
        function_port = node.inp(len(node_data.op.signature.body.input))
        sources = list(module.linked_ports(function_port))
        assert len(sources) == 1
        count += sources[0].node == target
    return count


class ProbePackage:
    def __init__(self, payload: bytes):
        self.payload = payload

    def to_bytes(self) -> bytes:
        return self.payload

    @classmethod
    def from_bytes(cls, payload: bytes):
        return cls(payload)


def test_link_alpha_function_uses_package_serialization_protocol(monkeypatch):
    wrapper = ProbePackage(b"wrapper")
    calls = []

    def native(wrapper_bytes, implementation, symbol):
        calls.append((wrapper_bytes, implementation, symbol))
        return b"linked"

    monkeypatch.setattr(alphalang, "_link_alpha_function", native)
    linked = alphalang.link_alpha_function(wrapper, "alpha", symbol="kernel")

    assert calls == [(b"wrapper", "alpha", "kernel")]
    assert isinstance(linked, ProbePackage)
    assert linked.payload == b"linked"
    assert wrapper.payload == b"wrapper"


def test_link_alpha_function_requires_package_protocol(monkeypatch):
    monkeypatch.setattr(alphalang, "_link_alpha_function", lambda *_: b"linked")

    with pytest.raises(AttributeError, match="to_bytes"):
        alphalang.link_alpha_function(object(), "alpha")


def test_link_alpha_function_preserves_native_value_error(monkeypatch):
    def native(*_):
        raise ValueError("signature mismatch for 'foo'")

    monkeypatch.setattr(alphalang, "_link_alpha_function", native)

    with pytest.raises(ValueError, match="signature mismatch for 'foo'"):
        alphalang.link_alpha_function(ProbePackage(b"wrapper"), "alpha")


@requires_guppy
def test_links_alpha_into_guppy_declaration_and_preserves_calls():
    from guppylang import guppy
    from guppylang.library import link_name
    from guppylang.std.builtins import array
    from hugr.ops import FuncDefn

    @guppy.declare
    @link_name("foo")
    def alpha_decl() -> array[bool, 4]: ...

    @guppy
    def main() -> tuple[array[bool, 4], array[bool, 4]]:
        return alpha_decl(), alpha_decl()

    wrapper = main.compile()
    linked = alphalang.link_alpha_function(wrapper, alpha_bool_array_hugr(), "foo")

    assert linked is not wrapper
    assert linked.modules[0][linked.modules[0].entrypoint].op.f_name.endswith(".main")
    targets = named_functions(linked.modules[0], "foo")
    assert len(targets) == 1
    assert isinstance(linked.modules[0][targets[0]].op, FuncDefn)
    assert count_calls_to(linked.modules[0], targets[0]) == 2


@requires_guppy
def test_replaces_guppy_dummy_definition():
    from guppylang import guppy
    from guppylang.library import GuppyLibrary, link_name
    from guppylang.std.builtins import array
    from hugr.ops import DFG, FuncDefn

    @guppy
    @link_name("foo")
    def foo() -> array[bool, 4]:
        return array(False, False, False, False)

    @guppy
    @link_name("main")
    def main() -> array[bool, 4]:
        return foo()

    wrapper = GuppyLibrary.from_members(foo, main).compile()
    main_nodes = named_functions(wrapper.modules[0], "main")
    assert len(main_nodes) == 1
    wrapper.modules[0].entrypoint = main_nodes[0]
    linked = alphalang.link_alpha_function(wrapper, alpha_bool_array_hugr())

    targets = named_functions(linked.modules[0], "foo")
    assert len(targets) == 1
    assert isinstance(linked.modules[0][targets[0]].op, FuncDefn)
    assert count_calls_to(linked.modules[0], targets[0]) == 1
    assert any(
        isinstance(node_data.op, DFG) for _, node_data in linked.modules[0].items()
    )


@requires_guppy
def test_guppy_linking_reports_missing_symbol_and_signature_mismatch():
    from guppylang import guppy
    from guppylang.library import link_name
    from guppylang.std.builtins import array

    @guppy.declare
    @link_name("foo")
    def wrong_size() -> array[bool, 3]: ...

    @guppy
    def main() -> array[bool, 3]:
        return wrong_size()

    wrapper = main.compile()
    implementation = alpha_bool_array_hugr()
    with pytest.raises(ValueError, match="public symbol 'missing' was not found"):
        alphalang.link_alpha_function(wrapper, implementation, "missing")
    with pytest.raises(ValueError, match="signature mismatch for 'foo'"):
        alphalang.link_alpha_function(wrapper, implementation, "foo")
