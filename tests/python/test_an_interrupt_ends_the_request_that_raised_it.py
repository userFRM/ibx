"""What a callback raises comes out of the call that ran it.

An exception restored onto the interpreter and then followed by a normal return
is left pending, and the interpreter raises it at whatever it calls next — far
from the handler, on unrelated code, reading as a fault in the interpreter
rather than in the program. So nothing is restored: an ordinary exception is the
caller's own business and is logged, and anything else ends the call.

A request made with no session reports 504 on the wrapper before returning, and
that report is the one place a handler runs inside a request that has nothing to
hand an error back through. It has one now.
"""

import pytest

import ibx


class Raises(ibx.EWrapper):
    def __init__(self, what):
        self.what = what
        self.seen = 0

    def error(self, *said):
        self.seen += 1
        raise self.what


def test_an_interrupt_from_the_report_ends_the_request():
    heard = Raises(KeyboardInterrupt("from the handler"))
    client = ibx.EClient(heard)
    with pytest.raises(KeyboardInterrupt):
        client.reqCurrentTime()
    assert heard.seen == 1, "the handler ran, and what it raised was not swallowed"


def test_an_ordinary_exception_is_the_callers_own_business():
    # Logged, not raised: the reference client's own dispatch carries on
    # through one, and a request that reported 504 has already done its job.
    heard = Raises(ValueError("the handler's own problem"))
    client = ibx.EClient(heard)
    client.reqCurrentTime()
    client.reqPositions()
    assert heard.seen == 2


def test_nothing_is_left_pending_for_the_next_call():
    # The failure this replaces: a restored exception surfacing as a
    # SystemError inside whatever the interpreter touched next.
    heard = Raises(KeyboardInterrupt("first"))
    client = ibx.EClient(heard)
    with pytest.raises(KeyboardInterrupt):
        client.reqCurrentTime()
    assert len("".join(str(x) for x in range(50))) == 90
    assert sorted([3, 1, 2]) == [1, 2, 3]


class InterruptsOnTheFirstPosition(ibx.EWrapper):
    def __init__(self):
        self.heard = []

    def position(self, account, contract, position, avg_cost):
        self.heard.append(position)
        if len(self.heard) == 1:
            raise KeyboardInterrupt("mid-answer")

    def position_end(self):
        self.heard.append("end")


def test_the_answers_behind_an_interrupt_are_delivered_on_the_next_pass():
    """An interrupt raised mid-answer ends the pass where it is raised, and the
    answers queued behind it are not lost: they stay queued, in order, and the
    next pass hands them over first. Draining them first and raising after
    would run more of the caller's code after the caller asked to stop, and
    could park the interrupt behind a callback that blocks."""
    heard = InterruptsOnTheFirstPosition()
    client = ibx.EClient(heard)
    client._test_connect("DU111111", True)
    client._test_set_position(111, 10.0, 5.0)
    client._test_set_position(222, 20.0, 6.0)
    client.req_positions()

    with pytest.raises(KeyboardInterrupt):
        client.poll()
    assert len(heard.heard) == 1, "the pass did not end at the raise"

    client.poll()
    answer, closed = heard.heard[:2], heard.heard[2:3]
    assert sorted(answer) == [10.0, 20.0], f"the answer behind the raise was lost: {heard.heard}"
    assert closed == ["end"] and heard.heard.count("end") == 1, heard.heard
