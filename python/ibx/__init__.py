"""A JVM-free Interactive Brokers client.

Two surfaces sit here, and they are not alternatives to each other:

``EClient``/``EWrapper`` carry the reference client's shape — a request under an
id, an answer later on a callback — so a program written against that client
runs here unchanged.

``Client`` carries the shape of the widely used asynchronous wrapper around it,
and is also exported as ``IB`` for a program that expects that name:
methods that send a question and hand the answer back. It is a facade over
``EClient``, not a second client, so the two see the same session.
"""

import inspect

from .ibx import *  # noqa: F401,F403
from .ibx import __doc__ as _ext_doc  # noqa: F401

from ._ib import Client  # noqa: F401

#: The session under the name the widely used asynchronous wrapper gives it, so
#: a program written against that one finds what it is looking for. The same
#: class either way.
IB = Client
from ._settings import UNAVAILABLE, configure, describe, settings  # noqa: F401

# The plain objects and constants a program written against the reference
# client imports on its first line. Read by attribute here, so the shape is the
# whole of what they are.
from ._reference_shapes import (  # noqa: F401
    DOUBLE_INFINITY,
    INFINITY_STR,
    MAX_MSG_LEN,
    NO_VALID_ID,
    UNSET_DECIMAL,
    UNSET_DOUBLE,
    UNSET_INTEGER,
    UNSET_LONG,
    COMPETE_AGAINST_BEST_OFFSET_UP_TO_MID,
    AccountSummaryTags,
    DeltaNeutralContract,
    ExecutionFilter,
    FaDataTypeEnum,
    FamilyCode,
    HistogramData,
    HistogramDataList,
    HistoricalSession,
    ListOfContractDescription,
    ListOfDepthExchanges,
    ListOfFamilyCode,
    ListOfHistoricalSessions,
    ListOfHistoricalTick,
    ListOfHistoricalTickBidAsk,
    ListOfHistoricalTickLast,
    ListOfNewsProviders,
    ListOfOrder,
    ListOfPriceIncrements,
    MarketDataTypeEnum,
    Object,
    OrderCancel,
    OrderComboLeg,
    OrderCondition,
    OrderId,
    RealTimeBar,
    ScanData,
    ScannerSubscription,
    SetOfFloat,
    SetOfString,
    SmartComponentMap,
    TagValueList,
    TickerId,
    TickType,
    WshEventData,
    decimalMaxString,
    floatMaxString,
    getEnumTypeName,
    getTimeStrFromMillis,
    intMaxString,
    iswrapper,
    longMaxString,
)


def _reference_name(ours: str) -> str:
    """What the reference client calls the callback this one calls `ours`.

    A capital after each underscore, except for the three it spells with the
    letters run together. The engine keeps the same three, beside the code that
    delivers a callback under that client's name.
    """
    run_together = {
        "real_time_bar": "realtimeBar",
        "receive_fa": "receiveFA",
        "replace_fa_end": "replaceFAEnd",
        "req_pnl": "reqPnL",
        "cancel_pnl": "cancelPnL",
        "req_pnl_single": "reqPnLSingle",
        "cancel_pnl_single": "cancelPnLSingle",
        "request_fa": "requestFA",
        "replace_fa": "replaceFA",
    }
    if ours in run_together:
        return run_together[ours]
    head, *rest = ours.split("_")
    return head + "".join(word[:1].upper() + word[1:] for word in rest)


def _answer_to_both_spellings(cls) -> None:
    """Put the reference client's spelling on the class, not only on instances.

    Resolved on the instance alone, `super().tickPrice(...)` does not find it:
    `super` walks the remaining classes' own contents and never asks an
    instance hook. A program written against that client calls `super()` first
    in nearly every override — its own sample does — so the rest of each
    override never ran, and the dispatch loop logged the failure and carried on
    without it.
    """
    for ours in list(vars(cls)):
        if ours.startswith("_"):
            continue
        theirs = _reference_name(ours)
        if theirs == ours or hasattr(cls, theirs):
            continue
        under_ours = getattr(cls, ours)
        # A value read off the instance, not a call: `clientId` is what
        # `client_id` answers. Wrapped as a call, reading it handed back the
        # wrapper itself.
        if not callable(under_ours):
            setattr(cls, theirs, property(
                lambda self, ours=ours: getattr(self, ours), doc=under_ours.__doc__,
            ))
            continue
        setattr(cls, theirs, _standing_in_front_of(under_ours, cls, theirs))


def _one_word(name: str) -> str:
    """A parameter name with nothing in it but its letters, in lower case.

    `useRTH` and `use_rth` are the same parameter, and so are `acctCode` and
    `acct_code`; the underscores and the capitals are the only difference, and
    an acronym makes the capitals unguessable in either direction.
    """
    return name.replace("_", "").lower()


#: Parameters this client names something else entirely, by letters alone. The
#: rest of the reference client's names differ from ours only in underscores and
#: capitals, which `_one_word` settles; these three are different words, so
#: nothing but a list of them will do. A name absent from the method it is
#: passed to is handed on untouched, and the method refuses it as before.
_THEIR_WORD_FOR_IT = {
    "tickerid": "req_id",
    "fadata": "fa_data_type",
    "implvoloptions": "implied_vol_options",
}


def _under_our_names(method):
    """What each of this method's parameters is called, by its letters alone.

    Read off the method itself rather than composed from a rule: a rule that
    turns `use_rth` into `useRth` misses `useRTH`, which is what the reference
    client calls it and therefore what a caller writes.

    Empty where the method states no parameters this can read, in which case a
    keyword is passed on untouched and the method itself refuses it.
    """
    try:
        params = inspect.signature(method).parameters
    except (TypeError, ValueError):
        return {}
    ours = {_one_word(name): name for name in params if name != "self"}
    for theirs, mine in _THEIR_WORD_FOR_IT.items():
        if mine in params:
            ours[theirs] = mine
    return ours


def _standing_in_front_of(method, cls, theirs: str):
    """The reference spelling, as a function that calls the method.

    Not the method under a second name. Delivery tries the reference spelling
    first, so a base class answering to it stands in front of a subclass that
    overrode only this client's spelling — and the engine has to tell the base's
    answer from the caller's to pass it over. A bound Python function carries
    `__func__` and the method's own bound form carries nothing that names it,
    so the function is what makes the two tellable apart.

    It also takes a keyword under the reference client's spelling of it. A
    program written against that client passes them by name — its own
    documentation gives ten of them for one request — and every one of those
    names was refused, so calling the reference spelling of a method with the
    reference spelling of its arguments raised.
    """
    ours_by_letters = _under_our_names(method)

    def theirs_calls_ours(self, *args, **kwargs):
        if kwargs:
            kwargs = {
                ours_by_letters.get(_one_word(given), given): value
                for given, value in kwargs.items()
            }
        return method(self, *args, **kwargs)

    theirs_calls_ours.__name__ = theirs
    theirs_calls_ours.__qualname__ = f"{cls.__name__}.{theirs}"
    theirs_calls_ours.__doc__ = method.__doc__
    # What `inspect.signature` reads the method's own through.
    theirs_calls_ours.__wrapped__ = method
    return theirs_calls_ours


for _surface in (EWrapper, EClient):  # noqa: F405
    _answer_to_both_spellings(_surface)
del _surface

# The reference client's own module names, laid over what this package
# publishes, so its import lines resolve under the one rename a person would
# guess at. Bound as attributes as well as registered, because a program writes
# both `from ibx import wrapper` and `from ibx.wrapper import EWrapper`.
from . import _layout as _layout_module  # noqa: E402

globals().update(_layout_module.install(dict(globals())))
del _layout_module

# What `from ibx import *` brings. The extension module is bound on this
# package by the star-import above, so `dir()` names it too: left in, the star
# import rebinds the caller's own `ibx` to that submodule and `ibx.IB` stops
# existing. The layout's modules go the same way — `from ibapi import *` brings
# none of them there, and a script that had its own `order` or `contract` would
# lose it to ours. So: no modules, and nothing this file merely imported to do
# its own work.
import types as _types

__all__ = [
    n for n, held in sorted(globals().items())
    if not n.startswith("_")
    and n != "ibx"
    and not isinstance(held, _types.ModuleType)
]
