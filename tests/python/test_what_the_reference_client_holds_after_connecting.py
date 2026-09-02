"""What a program written against the reference client reads off its client.

Its sample reads `asynchronous` in `connectAck`, prints `serverVersion()` and
`twsConnectionTime()` once connected, calls `setConnectOptions` before, and
gives the client id as `clientId`. Each is answered with what this client has.
Where the reference client's value belongs to the gateway process it speaks
to, of which there is none here, the answer is `None` rather than a stand-in.
"""

import pytest
from conftest import NotConnectedProbe
from ibx import EClient, EWrapper


def test_the_connection_states_are_the_reference_clients():
    assert (EClient.DISCONNECTED, EClient.CONNECTING, EClient.CONNECTED) == (0, 1, 2)


def test_conn_state_follows_the_session():
    c = EClient(EWrapper())
    assert c.connState == EClient.DISCONNECTED
    c._test_connect()
    assert c.connState == EClient.CONNECTED
    c.disconnect()
    assert c.conn_state == EClient.DISCONNECTED


def test_the_client_id_is_the_sessions_and_none_without_one():
    c = EClient(EWrapper())
    assert c.clientId is None
    c._test_connect()
    c._test_set_client_id(7)
    assert c.clientId == 7
    c.disconnect()
    assert c.client_id is None


def test_what_belongs_to_a_gateway_is_absent():
    c = EClient(EWrapper())
    c._test_connect()
    assert c.serverVersion() == 217, "the level this client implements; None only before a session"
    assert c.conn is None
    assert c.port is None
    assert c.asynchronous is False


def test_host_and_logon_time_are_none_without_a_venue():
    """A test session has no venue behind it, so nothing named a server or
    stamped the logon; with no session at all there is nothing either."""
    c = EClient(EWrapper())
    assert c.host is None and c.twsConnectionTime() is None
    c._test_connect()
    assert c.host is None and c.tws_connection_time() is None


def test_connect_options_are_taken():
    c = EClient(EWrapper())
    assert c.setConnectOptions("+PACEAPI") is None


def test_start_api_reports_no_session_and_otherwise_nothing():
    probe = NotConnectedProbe()
    c = EClient(probe)
    c.startApi()
    assert probe.not_connected
    probe.errors.clear()
    c._test_connect()
    c.startApi()
    c._test_dispatch_once()
    assert probe.errors == []


def test_check_connected_raises_only_with_a_session():
    c = EClient(EWrapper())
    assert c.checkConnected() is None
    c._test_connect()
    with pytest.raises(RuntimeError, match="Already connected"):
        c.checkConnected()


def test_reset_closes_the_session_and_leaves_the_client_reusable():
    c = EClient(EWrapper())
    c.reset()
    c._test_connect()
    c.reset()
    assert not c.is_connected()
    assert c.connState == EClient.DISCONNECTED and c.clientId is None
    c._test_connect()
    assert c.is_connected()


def test_the_client_id_is_taken_under_the_reference_spelling():
    """The spelling reaches `connect`: what fails is the logon, not the call."""
    c = EClient(EWrapper())
    with pytest.raises(RuntimeError, match="Connection failed"):
        c.connect("nothing.invalid", 7497, clientId=0, username="u", password="p")
    assert not c.is_connected()


def test_the_client_id_given_under_both_spellings_is_refused():
    c = EClient(EWrapper())
    with pytest.raises(TypeError, match="both spellings"):
        c.connect("nothing.invalid", 7497, client_id=1, clientId=2, username="u", password="p")
