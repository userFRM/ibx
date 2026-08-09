"""A JVM-free Interactive Brokers client.

Two surfaces sit here, and they are not alternatives to each other:

``EClient``/``EWrapper`` carry the reference client's shape — a request under an
id, an answer later on a callback — so a program written against that client
runs here unchanged.

``IB`` carries the shape of the widely used asynchronous wrapper around it:
methods that send a question and hand the answer back. It is a facade over
``EClient``, not a second client, so the two see the same session.
"""

from .ibx import *  # noqa: F401,F403
from .ibx import __doc__ as _ext_doc  # noqa: F401

from ._ib import IB  # noqa: F401
from ._settings import UNAVAILABLE, configure, describe, settings  # noqa: F401

__all__ = [n for n in dir() if not n.startswith("_")]
