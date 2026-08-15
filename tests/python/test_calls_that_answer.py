"""Calls that send a question and hand back the answer.

The callback shape is right for a program with its own event loop and wrong for
asking one question. These check the shape that answers directly.
"""

import pytest

import ibx


class Wrapper(ibx.EWrapper):
    pass


def connected():
    c = ibx.EClient(Wrapper())
    c._test_connect("DU0000000")
    return c


def spy():
    con = ibx.Contract()
    con.symbol = "SPY"
    con.secType = "STK"
    con.exchange = "SMART"
    con.currency = "USD"
    return con


def test_a_lookup_hands_back_what_the_venue_said():
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_contract_details(req_id, 756733, "SPY")
    c._test_push_contract_details_end(req_id)

    found = c.contract_details(spy())
    assert len(found) == 1
    assert found[0].contract.conId == 756733
    assert found[0].contract.symbol == "SPY"


def test_qualifying_fills_in_the_contract_id():
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_contract_details(req_id, 756733, "SPY")
    c._test_push_contract_details_end(req_id)

    assert c.qualify_contract(spy()).conId == 756733


def test_a_description_matching_nothing_is_refused_not_guessed():
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_contract_details_end(req_id)

    with pytest.raises(ValueError):
        c.qualify_contract(spy())


def test_a_description_matching_two_contracts_is_refused_not_picked():
    """The same symbol on the same venue exists in more than one currency.

    Picking one silently is how an order reaches the wrong contract.
    """
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_contract_details(req_id, 756733, "SPY")
    c._test_push_contract_details(req_id, 999999, "SPY")
    c._test_push_contract_details_end(req_id)

    with pytest.raises(ValueError):
        c.qualify_contract(spy())


def test_one_question_does_not_take_another_questions_answer():
    """A dispatch loop beside this keeps its own answers."""
    c = connected()
    mine = c._test_peek_ask_id()
    c._test_push_contract_details(9999, 111111, "OTHER")
    c._test_push_contract_details(mine, 756733, "SPY")
    c._test_push_contract_details_end(mine)

    found = c.contract_details(spy())
    assert [d.contract.conId for d in found] == [756733]

    # The other request's answer is still there for whoever asked it.
    c._test_push_contract_details_end(9999)
    c._test_dispatch_once()


def test_a_question_asked_of_a_client_that_is_not_connected_says_so():
    c = ibx.EClient(Wrapper())
    with pytest.raises(RuntimeError):
        c.contract_details(spy())


def test_bars_come_back_as_a_list_not_one_callback_at_a_time():
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_historical_data(
        req_id,
        [("20260101 09:30:00", 1.0, 2.0, 0.5, 1.5, 100),
         ("20260101 09:31:00", 1.5, 2.5, 1.0, 2.0, 200)],
        True,
    )
    bars = c.historical_data(spy(), "", "1 D", "1 min", "TRADES")
    assert [b.close for b in bars] == [1.5, 2.0]
    assert bars[0].volume == 100


def test_a_series_answered_in_parts_is_not_cut_at_the_first_part():
    """A part that is not the last says so. Stopping on it returns a short
    series and nothing in the series says it is short."""
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_historical_data(req_id, [("t1", 1.0, 1.0, 1.0, 1.0, 1)], False)
    c._test_push_historical_data(req_id, [("t2", 2.0, 2.0, 2.0, 2.0, 2)], True)
    bars = c.historical_data(spy(), "", "1 D", "1 min", "TRADES")
    assert [b.close for b in bars] == [1.0, 2.0]


def test_the_earliest_data_comes_back_as_a_value():
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_head_timestamp(req_id, "19930129 14:30:00")
    assert c.head_timestamp(spy()) == "19930129 14:30:00"


def test_a_refusal_quotes_the_venue_rather_than_reporting_a_timeout():
    """"The venue said no" and "nothing came" are different facts."""
    c = connected()
    req_id = c._test_peek_ask_id()
    c._test_push_historical_error(req_id, 162, "Historical Market Data Service error")
    with pytest.raises(RuntimeError, match="Historical Market Data Service error"):
        c.historical_data(spy(), "", "1 D", "1 min", "TRADES")


def test_a_dispatch_loop_running_beside_an_ask_does_not_eat_its_answer():
    """The facade keeps a pump running while its calls ask questions.

    Both read the same queues. Without arbitration, whichever runs first
    empties them and the other reports that nothing arrived.
    """
    c = connected()
    mine = c._test_peek_ask_id()
    c._test_push_contract_details(mine, 756733, "SPY")
    c._test_push_contract_details_end(mine)

    # A full dispatch pass, exactly as a pump would run it.
    c._test_dispatch_once()

    found = c.contract_details(spy())
    assert [d.contract.conId for d in found] == [756733]


def test_a_dispatch_loop_still_delivers_a_callers_own_request():
    seen = []

    class W(ibx.EWrapper):
        def contractDetails(self, reqId, details):
            seen.append((reqId, details.contract.conId))

    c = ibx.EClient(W())
    c._test_connect("DU0000000")
    c._test_push_contract_details(42, 111111, "AAPL")
    c._test_dispatch_once()
    assert seen == [(42, 111111)]
