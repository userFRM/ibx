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
