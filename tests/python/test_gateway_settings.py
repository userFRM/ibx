"""The settings that used to live in the gateway's own file."""

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


def test_the_settings_actually_reach_the_client():
    """A setting that reads back but changes nothing is decoration."""
    import os

    ibx.configure(market_data_host="example.invalid")
    assert os.environ["IBX_FARM_HOST"] == "example.invalid"
    ibx.configure(market_data_host=None)
