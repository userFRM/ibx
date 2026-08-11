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


def test_a_session_states_its_own_settings():
    """Settings belong to the session that stated them, not to the process.

    They used to be written into the process environment as a session opened,
    so a second session in one process silently retargeted the first: whichever
    connected last decided the time zone, the build, and where the market-data
    connection went for both.
    """
    client = ibx.EClient(ibx.EWrapper())
    # Refused before anything is sent, so a misspelling cannot open a session
    # configured differently from the way it was written.
    try:
        client.connect(username="u", password="p", settings={"tiemzone": "UTC"})
    except RuntimeError as refusal:
        assert "no such setting: tiemzone" in str(refusal)
    else:
        raise AssertionError("a setting that is not a setting was accepted")

    # And the process is not touched by a session stating one.
    assert ibx.settings()["timezone"] is None
