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

# The extension module is bound on this package by the star-import above, so
# `dir()` names it too. Left in, `from ibx import *` rebinds the caller's own
# `ibx` to that submodule and `ibx.IB` stops existing.
__all__ = [n for n in dir() if not n.startswith("_") and n != "ibx"]
