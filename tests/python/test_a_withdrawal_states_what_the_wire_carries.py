"""A withdrawal takes the object the reference client states it with.

That client's `cancelOrder` takes an `OrderCancel` and its `reqGlobalCancel`
takes one too — when a person entered the withdrawal, on whose authority, and
whether a person entered it at all. Passing one raised here, because the second
argument was a bare time string.

A cancel on this wire names five fields and none of those is among them, so
what a caller states in that object cannot travel. The order still comes back
and the caller is told the annotation did not go with it. Refusing the
withdrawal outright left a live order working because a record could not be
filed, which is the worse of the two — and the client this one stands in for
withdraws it: it states all three on every cancel it sends.
"""

import ibx


class Heard(ibx.EWrapper):
    def __init__(self):
        self.refusals = []

    def error(self, *said):
        # Kept whole: the reference client's callback carries five arguments
        # and this one only reads the sentence out of them.
        self.refusals.append(said)


def _client():
    heard = Heard()
    client = ibx.EClient(heard)
    client._test_connect("DU0000000")
    # A withdrawal names an order this client is working, or it is answered
    # rather than sent. What is under test here is the annotation the
    # withdrawal carries, so the orders it names are placed first.
    for order_id in (1, 2):
        client._test_track_order(order_id, 0, "SPY", "BUY", 1.0, 100.0)
    return client, heard


def _said(client, heard):
    client._test_dispatch_once()
    return [
        held for said in heard.refusals for held in said if isinstance(held, str)
    ]


def _withdrew(client):
    return [cmd for cmd in client._test_take_commands() if "Cancel" in cmd]


def test_a_withdrawal_that_states_nothing_goes_through():
    client, heard = _client()
    client.cancelOrder(1, ibx.OrderCancel())
    assert _withdrew(client), "the order comes back"
    assert not [t for t in _said(client, heard) if "withdrawal states" in t]


def test_the_object_is_taken_where_a_bare_time_was_taken_before():
    # The spelling this client had is still answered, so nothing that worked
    # stops working.
    client, heard = _client()
    client.cancelOrder(1, "")
    client.cancel_order(2)
    assert len(_withdrew(client)) == 2
    assert not [t for t in _said(client, heard) if "withdrawal states" in t]


def test_a_time_the_wire_cannot_carry_is_said_and_the_order_still_comes_back():
    client, heard = _client()
    withdrawal = ibx.OrderCancel()
    withdrawal.manualOrderCancelTime = "20260902-14:30:00"
    client.cancelOrder(1, withdrawal)

    assert _withdrew(client), "a record with nowhere to go does not keep an order working"
    said = [t for t in _said(client, heard) if "withdrawal states" in t]
    assert said, heard.refusals
    assert "a time" in said[0] and "no field for it" in said[0]


def test_an_operator_the_wire_cannot_carry_is_said():
    client, heard = _client()
    withdrawal = ibx.OrderCancel()
    withdrawal.extOperator = "someone"
    client.cancelOrder(1, withdrawal)
    assert _withdrew(client)
    assert [t for t in _said(client, heard) if "an operator" in t]


def test_who_entered_it_is_said_and_the_unset_value_is_not():
    client, heard = _client()
    # The number an integer nobody set carries is not a statement.
    left_alone = ibx.OrderCancel()
    assert left_alone.manualOrderIndicator == ibx.UNSET_INTEGER
    client.cancelOrder(1, left_alone)
    assert not [t for t in _said(client, heard) if "withdrawal states" in t]

    stated = ibx.OrderCancel()
    stated.manualOrderIndicator = 1
    client.cancelOrder(2, stated)
    assert len(_withdrew(client)) == 2, "both orders come back"
    assert [t for t in _said(client, heard) if "who entered it" in t]


def test_the_global_withdrawal_says_the_same_and_still_withdraws():
    client, heard = _client()
    withdrawal = ibx.OrderCancel()
    withdrawal.manualOrderCancelTime = "20260902-14:30:00"
    client.reqGlobalCancel(withdrawal)
    assert [t for t in _said(client, heard) if "withdrawal states" in t]
