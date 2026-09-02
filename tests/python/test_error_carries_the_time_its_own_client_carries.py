"""`error` has the parameter the reference client's wrapper has, where it has it.

That wrapper's signature is
``error(reqId, errorTime, errorCode, errorString, advancedOrderRejectJson="")``
and every one of its call sites passes five. The arity does not vary: only the
value does, and its decoder passes zero for a session speaking a protocol older
than the one that added the field.

Given four, a handler written that way bound `errorTime` to the code and
`errorCode` to the message, silently — which is why a handler answering 1100 by
reconnecting never fired.
"""

from ibx import EClient, EWrapper


class Records(EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []

    def error(self, reqId, errorTime, errorCode, errorString, advancedOrderRejectJson=""):
        self.errors.append((reqId, errorTime, errorCode, errorString))


def test_a_refusal_this_client_raised_carries_a_clock_reading():
    """What that client stamps: trouble raised before anything reached the venue.

    Its own `NOT_CONNECTED` goes out as
    ``error(NO_VALID_ID, currentTimeMillis(), code, message)``.
    """
    wrapper = Records()
    client = EClient(wrapper)
    client.req_current_time()

    (req_id, error_time, code, message), = wrapper.errors
    assert req_id == -1
    assert code == 504, "the code is in the slot the code goes in"
    assert message == "Not connected"
    assert error_time > 1_700_000_000_000, (
        f"a clock reading in milliseconds, got {error_time}"
    )


def test_the_base_wrapper_takes_the_five_the_other_client_takes():
    EWrapper().error(-1, 0, 504, "test", "")
