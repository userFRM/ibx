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
        self.positions = []
        self.ends = 0

    def position(self, account, contract, position, avg_cost):
        self.positions.append(position)
        if len(self.positions) == 1:
            raise RuntimeError("what the caller wrote is the caller's problem")

    def position_end(self):
        self.ends += 1


def test_the_rest_of_the_answer_and_its_end_still_arrive():
    wrapper = RaisesOnTheFirstPosition()
    client = EClient(wrapper)
    client._test_connect("DU111111", True)
    client._test_set_position(111, 10.0, 5.0)
    client._test_set_position(222, 20.0, 6.0)

    client.req_positions()

    assert len(wrapper.positions) == 2, (
        f"the answer behind the raise is still owed, got {wrapper.positions}"
    )
    assert wrapper.ends == 1, "and the end that closes the batch still arrives"
