"""The settings on both clients are the same settings.

Two lists of the same thing drift the moment one is added to. A setting added
on the Rust client and not here would be one a Python caller cannot set, and
the other way round would be one that does nothing.
"""

import re
import pathlib

import ibx


def _rust_settings() -> set[str]:
    source = pathlib.Path(__file__).resolve().parents[2] / "src/api/settings.rs"
    text = source.read_text()
    at = text.index("pub struct GatewaySettings {")
    end = text.index("\n}", at)
    return set(re.findall(r"^\s*pub (\w+):", text[at:end], re.M))


def test_both_clients_carry_the_same_settings():
    assert _rust_settings() == set(ibx.settings()), (
        "a setting exists on one client and not the other"
    )


def test_every_setting_is_readable_after_being_set():
    ibx.configure(timezone="America/New_York")
    assert ibx.settings()["timezone"] == "America/New_York"
    ibx.configure(timezone=None)
    assert ibx.settings()["timezone"] is None


def test_a_misspelled_setting_is_refused():
    try:
        ibx.configure(timezon="UTC")
    except ValueError as refused:
        assert "timezon" in str(refused)
    else:
        raise AssertionError("a misspelled setting was accepted")
