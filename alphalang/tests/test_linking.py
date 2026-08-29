import pytest

import alphalang


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