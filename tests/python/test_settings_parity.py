"""The settings on both clients are the same settings.

Two lists of the same thing drift the moment one is added to. A setting added
on the Rust client and not here would be one a Python caller cannot set, and
the other way round would be one that does nothing.
"""

import re
import pathlib

import ibx


def _rust_settings() -> set[str]:
    source = pathlib.Path(__file__).resolve().parents[2] / "src/settings.rs"
    text = source.read_text()
    at = text.index("pub struct GatewaySettings {")
    end = text.index("\n}", at)
    return set(re.findall(r"^\s*pub (\w+):", text[at:end], re.M))


def _rust_unavailable() -> set[str]:
    source = pathlib.Path(__file__).resolve().parents[2] / "src/settings.rs"
    text = source.read_text()
    at = text.index("pub const UNAVAILABLE:")
    end = text.index("];", at)
    return set(re.findall(r'\(\s*"([\w.]+)"', text[at:end]))


def test_both_clients_carry_the_same_settings():
    assert _rust_settings() == set(ibx.settings()), (
        "a setting exists on one client and not the other"
    )


def test_both_clients_name_the_same_settings_as_unavailable():
    """A setting with no counterpart is a statement, and both clients make it.

    Comparing only what is carried leaves the other list free to drift: three
    names sat on the Rust list and not on this one, so a Python caller asking
    about them was told nothing rather than why. What a caller cannot have is
    as much a part of the surface as what they can.
    """
    assert _rust_unavailable() == set(ibx.UNAVAILABLE), (
        "a setting is recorded as having no counterpart on one client and not the other"
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

    Written into the process environment as a session opens, a second session
    in one process silently retargets the first: whichever connects last
    decides the time zone, the build, and where the market-data connection
    goes for both.
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
