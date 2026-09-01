"""Every name the reference client uses reaches the call it names.

A capital starts a word, which is enough for every name either side spells out
and not enough for the ones that run their letters together: split that way,
`reqPnL` is `req_pn_l` and names nothing, so a program asking this client for a
running profit was told it carries none.

The names are read from the table the reference pages are generated from rather
than typed here, so a call added there is covered without a second edit.
"""

import importlib.util
import pathlib

import pytest
from ibx import EClient, EWrapper

ROOT = pathlib.Path(__file__).resolve().parents[2]


def _reference_names():
    spec = importlib.util.spec_from_file_location(
        "gen_api_docs", ROOT / "scripts" / "gen_api_docs.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return [(ours, theirs) for _, ours, theirs in module.IBAPI_ECLIENT]


@pytest.mark.parametrize("ours,theirs", _reference_names())
def test_a_call_answers_to_the_name_the_other_client_gives_it(ours, theirs):
    client = EClient(EWrapper())
    if not hasattr(client, ours):
        pytest.skip(f"{ours} is not carried by this client")
    assert hasattr(client, theirs), (
        f"a program written against the reference client calls {theirs}, "
        f"and this client carries it as {ours}"
    )


@pytest.mark.parametrize("surface", [EClient, EWrapper])
def test_no_two_names_read_the_same_with_the_underscores_taken_out(surface):
    """What decides a run-together name is the letters alone.

    Two names differing only in where the underscores fall would leave that
    undecided, and the one answered would be whichever the listing named first.
    """
    obj = surface(EWrapper()) if surface is EClient else surface()
    seen: dict[str, str] = {}
    for name in dir(obj):
        if name.startswith("_"):
            continue
        flat = name.replace("_", "").lower()
        assert flat not in seen, (
            f"{name} and {seen[flat]} both read as {flat!r} with the "
            f"underscores taken out"
        )
        seen[flat] = name


def test_a_name_that_names_nothing_is_still_refused():
    client = EClient(EWrapper())
    with pytest.raises(AttributeError, match="'EClient' object has no attribute"):
        client.reqSomethingNobodyCarries


def test_the_object_still_lists_what_it_carries():
    """`dir()` asks the object for `__dict__`, which it does not have.

    Answered by the same route as a reference name, that question is asked
    again for every answer, and listing the object never returns.
    """
    assert len(dir(EClient(EWrapper()))) > 50


# The callbacks the reference client spells with its letters run together. The
# table the reference pages are generated from records only this client's names
# for callbacks, so unlike the calls above these are named here.
RUN_TOGETHER_CALLBACKS = [
    ("real_time_bar", "realtimeBar"),
    ("receive_fa", "receiveFA"),
    ("replace_fa_end", "replaceFAEnd"),
]


@pytest.mark.parametrize("ours,theirs", RUN_TOGETHER_CALLBACKS)
def test_a_callback_answers_to_the_name_the_other_client_gives_it(ours, theirs):
    wrapper = EWrapper()
    assert hasattr(wrapper, ours)
    assert hasattr(wrapper, theirs), (
        f"a wrapper written against the reference client declares {theirs}, "
        f"and this client declares it as {ours}"
    )
