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

from .ibx import *  # noqa: F401,F403
from .ibx import __doc__ as _ext_doc  # noqa: F401

from ._ib import Client  # noqa: F401

#: The session under the name the widely used asynchronous wrapper gives it, so
#: a program written against that one finds what it is looking for. The same
#: class either way.
IB = Client
from ._settings import UNAVAILABLE, configure, describe, settings  # noqa: F401


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
        if theirs != ours and not hasattr(cls, theirs):
            setattr(cls, theirs, getattr(cls, ours))


# The client only. Putting them on the wrapper too breaks how a callback is
# delivered: the engine looks the reference spelling up first, and with the
# base class answering to it, a subclass that overrode the name this client
# uses stopped being reached at all. Making `super().tickPrice(...)` work on
# the wrapper needs the delivery to resolve against the subclass before the
# base, which is a change to the engine rather than a name put on a class.
_answer_to_both_spellings(EClient)  # noqa: F405

# The extension module is bound on this package by the star-import above, so
# `dir()` names it too. Left in, `from ibx import *` rebinds the caller's own
# `ibx` to that submodule and `ibx.IB` stops existing.
__all__ = [n for n in dir() if not n.startswith("_") and n != "ibx"]
