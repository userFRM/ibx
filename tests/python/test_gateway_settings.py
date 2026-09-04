"""The settings a gateway keeps in its own configuration file."""

import pytest

import ibx


def test_a_setting_can_be_set_and_read_back():
    ibx.configure(timezone="America/New_York")
    assert ibx.settings()["timezone"] == "America/New_York"
    ibx.configure(timezone=None)
    assert ibx.settings()["timezone"] is None


def test_a_misspelled_setting_is_refused_not_dropped():
    """Silently ignoring it leaves a caller believing a session is configured
    a way it is not."""
    with pytest.raises(ValueError, match="no such setting"):
        ibx.configure(timezoen="UTC")


def test_every_setting_names_the_gateway_setting_it_stands_in_for():
    text = ibx.describe()
    for name in ibx.settings():
        assert name in text


def test_a_gateway_setting_with_no_counterpart_says_so_rather_than_vanishing():
    """Someone migrating will look for these."""
    assert "TrustedIPs" in ibx.UNAVAILABLE
    assert "LocalServerPort" in ibx.UNAVAILABLE
    assert "readonly" in ibx.UNAVAILABLE["ApiOnly"]


def test_a_setting_reaches_the_variable_the_client_reads_it_from():
    """A setting that reads back but changes nothing is decoration.

    This is the half of that a process can check on its own: the value lands
    where a session opening will look for it. What each one then does on the
    wire — the host a farm connection opens on, the fields a logon announces,
    the machine identity presented — is checked against the messages
    themselves, beside the code that composes them.
    """
    import os

    ibx.configure(market_data_host="example.invalid")
    assert os.environ["IBX_FARM_HOST"] == "example.invalid"
    ibx.configure(market_data_host=None)


def test_a_logging_setting_says_it_cannot_be_set_rather_than_storing_one():
    """A process has one logger and importing this client installs it.

    Stored, the value reads back as a log level that was set and did nothing.
    Both ways of stating a setting refuse it, and both name the one place it
    is read from.
    """
    before = ibx.settings()["log_level"]
    with pytest.raises(ValueError, match="IBX_LOG_LEVEL"):
        ibx.configure(log_level="debug")
    assert ibx.settings()["log_level"] == before, "and nothing was stored"

    # The same refusal on the other way in, so a caller does not find one door
    # open and the other shut.
    client = ibx.EClient(ibx.EWrapper())
    with pytest.raises(RuntimeError, match="IBX_LOG_DIR"):
        client.connect(username="u", password="p", settings={"log_dir": "/tmp/ibx"})


def test_a_setting_stated_beside_a_logging_one_is_not_half_applied():
    """The refusal comes before anything is stored, so a call that names both
    a logging setting and an ordinary one leaves neither set."""
    before = ibx.settings()["timezone"]
    with pytest.raises(ValueError):
        ibx.configure(timezone="America/New_York", log_queue=4096)
    assert ibx.settings()["timezone"] == before
