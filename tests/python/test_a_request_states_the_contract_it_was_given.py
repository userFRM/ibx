"""A request states what the caller stated, not a value filled in for them.

The reference client requires the underlying's type on an option-chain request
and holds no exchange on a condition nobody named one on. Defaulted here, a
caller who left either off was answered about something else and told nothing.

The book request's own contract is checked beside the engine command it makes,
in the Rust tests.
"""

import pytest

import ibx


def test_option_chains_need_the_underlying_type():
    c = ibx.EClient(ibx.EWrapper())
    with pytest.raises(TypeError):
        c.req_sec_def_opt_params(3, "SPY")


def test_a_condition_states_no_exchange_the_caller_did_not():
    for cond in (ibx.PriceCondition(con_id=1, price=1.0),
                 ibx.VolumeCondition(con_id=1, volume=1),
                 ibx.PercentChangeCondition(con_id=1, change_percent=1.0)):
        assert cond.exchange == "", f"{type(cond).__name__} invented {cond.exchange!r}"
