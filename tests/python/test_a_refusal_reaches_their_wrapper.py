"""A refusal reaches an attached ib_async wrapper in the shape it declares.

This engine states when the venue said it, as the current reference client
does. ib_async predates that argument and declares four, so a wrapper handed
five raises on the first error or notice of the session — and a callback that
raises there closes the session, which turned any refusal at all into a
disconnection.

Run: pytest tests/python/test_a_refusal_reaches_their_wrapper.py -v
"""

import inspect

import ib_async.wrapper

from ibx.ib_async import _LoopBound


def test_their_wrapper_still_declares_four_arguments():
    # If this ever changes, the adapter below is what has to change with it.
    named = list(inspect.signature(ib_async.wrapper.Wrapper.error).parameters)
    assert named == ["self", "reqId", "errorCode", "errorString", "advancedOrderRejectJson"]


def test_a_refusal_is_handed_over_without_the_time():
    class Theirs:
        def __init__(self):
            self.seen = []

        def error(self, reqId, errorCode, errorString, advancedOrderRejectJson):
            self.seen.append((reqId, errorCode, errorString, advancedOrderRejectJson))

    theirs = Theirs()
    # What this engine calls with: the request, when, the code, the text, the rest.
    _LoopBound(theirs).error(-1, 1786795200000, 2148, "the venue is going down at 17:00", "")

    assert theirs.seen == [(-1, 2148, "the venue is going down at 17:00", "")]


def test_the_advanced_field_is_carried_when_the_venue_states_one():
    class Theirs:
        def __init__(self):
            self.seen = []

        def error(self, reqId, errorCode, errorString, advancedOrderRejectJson):
            self.seen.append(advancedOrderRejectJson)

    theirs = Theirs()
    _LoopBound(theirs).error(7, 0, 201, "refused", '{"why":"margin"}')
    assert theirs.seen == ['{"why":"margin"}']
