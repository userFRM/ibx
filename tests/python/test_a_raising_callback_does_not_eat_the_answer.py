"""A callback that raises does not take the rest of the answer with it.

The dispatch loop already treats a raising callback this way: what the caller
wrote is the caller's problem, and the answers behind it are still owed. The
request methods delivered with `?` instead, so one raise abandoned every answer
behind it and the end-of-answer that closes the batch — and a program waiting
to be told the answer was complete waited for good. The exception also came
back out of the request call, which is somewhere the reference client never
raises one.
"""

import pytest
from ibx import EClient, EWrapper


class RaisesOnTheFirstPosition(EWrapper):
    def __init__(self):
        super().__init__()
        self.heard = []

    def position(self, account, contract, position, avg_cost):
        self.heard.append(position)
        if len(self.heard) == 1:
            raise RuntimeError("what the caller wrote is the caller's problem")

    def position_end(self):
        self.heard.append("end")


def test_the_rest_of_the_answer_and_its_end_still_arrive():
    wrapper = RaisesOnTheFirstPosition()
    client = EClient(wrapper)
    client._test_connect("DU111111", True)
    client._test_set_position(111, 10.0, 5.0)
    client._test_set_position(222, 20.0, 6.0)

    client.req_positions()
    client._test_dispatch_once()

    # The answer is what comes before its end. The feed follows on the same
    # pass with what moved before the ask, which is these holdings again —
    # see `req_positions`.
    # What is owed is both holdings and the end that closes them. Which
    # holding comes first is not: they are read out of a map keyed by
    # contract, so the venue's own order is not kept and asserting one makes
    # the test pass or fail on where a hash happened to put them.
    answer, closed = wrapper.heard[:2], wrapper.heard[2:3]
    assert sorted(answer) == [10.0, 20.0], (
        f"the answer behind the raise is still owed, got {wrapper.heard}"
    )
    assert closed == ["end"], f"and the end that closes it, got {wrapper.heard}"
    assert wrapper.heard.count("end") == 1, "and the end that closes the batch arrives once"
