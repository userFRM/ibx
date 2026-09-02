"""A withdrawal takes the object the reference client states it with.

That client's `cancelOrder` takes an `OrderCancel` and its `reqGlobalCancel`
takes one too — when a person entered the withdrawal, on whose authority, and
whether a person entered it at all. Passing one raised here, because the second
argument was a bare time string.

A cancel on this wire names five fields and none of those is among them. So the
object is taken, and a withdrawal that states one of the three is refused rather
than sent without it: taken and dropped, the order was withdrawn under nobody's
name while the caller had given one.
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
    return client, heard


def _said(client, heard):
    client._test_dispatch_once()
    return [
        held for said in heard.refusals for held in said if isinstance(held, str)
    ]


def test_a_withdrawal_that_states_nothing_goes_through():
    client, heard = _client()
    client.cancelOrder(1, ibx.OrderCancel())
    client.reqGlobalCancel(ibx.OrderCancel())
    assert not [t for t in _said(client, heard) if "withdrawal states" in t]


def test_the_object_is_taken_where_a_bare_time_was_taken_before():
    # The spelling this client had is still answered, so nothing that worked
    # stops working.
    client, heard = _client()
    client.cancelOrder(1, "")
    client.cancel_order(2)
    assert not [t for t in _said(client, heard) if "withdrawal states" in t]


def test_a_time_the_wire_cannot_carry_is_refused_not_dropped():
    client, heard = _client()
    withdrawal = ibx.OrderCancel()
    withdrawal.manualOrderCancelTime = "20260902-14:30:00"
    client.cancelOrder(1, withdrawal)
    said = [t for t in _said(client, heard) if "withdrawal states" in t]
    assert said, heard.refusals
    assert "a time" in said[0] and "no field for it" in said[0]


def test_an_operator_the_wire_cannot_carry_is_refused():
    client, heard = _client()
    withdrawal = ibx.OrderCancel()
    withdrawal.extOperator = "someone"
    client.cancelOrder(1, withdrawal)
    assert [t for t in _said(client, heard) if "an operator" in t]


def test_who_entered_it_is_refused_and_the_unset_value_is_not():
    client, heard = _client()
    # The number an integer nobody set carries is not a statement.
    left_alone = ibx.OrderCancel()
    assert left_alone.manualOrderIndicator == ibx.UNSET_INTEGER
    client.cancelOrder(1, left_alone)
    assert not [t for t in _said(client, heard) if "withdrawal states" in t]

    stated = ibx.OrderCancel()
    stated.manualOrderIndicator = 1
    client.cancelOrder(2, stated)
    assert [t for t in _said(client, heard) if "who entered it" in t]


def test_the_global_withdrawal_refuses_the_same_way():
    client, heard = _client()
    withdrawal = ibx.OrderCancel()
    withdrawal.manualOrderCancelTime = "20260902-14:30:00"
    client.reqGlobalCancel(withdrawal)
    assert [t for t in _said(client, heard) if "withdrawal states" in t]
