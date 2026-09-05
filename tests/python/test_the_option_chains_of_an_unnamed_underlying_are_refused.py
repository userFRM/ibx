"""The chain is asked for by the id of the contract the options are on.

Sent with none, the venue's answer named the real id and matched nothing
waiting here, so the caller waited out the answer to a question the other
surface refuses at once, with the reason.
"""

import pytest

import ibx


def test_the_option_chains_of_an_unnamed_underlying_are_refused():
    c = ibx.EClient(ibx.EWrapper())
    with pytest.raises(RuntimeError, match="qualify it first"):
        c.option_chains("SPY", "", "STK", 0)
