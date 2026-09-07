"""A histogram reaches ib_async as their record, whatever shape this engine states.

Their wrapper reads `item.price` and `item.count` (`ib_async/wrapper.py`), and
this engine states a bucket the way the reference client does — `price` and
`size`. The bridge used to hand an entry straight through whenever it already
carried a `price`, which was right only while this engine handed back a plain
pair: the moment it began stating a record of its own, that shortcut forwarded
an object naming a field their wrapper does not read, and every histogram raised
inside their callback.
"""

import ibx
from ibx.ib_async import _LoopBound


class Caught:
    def __init__(self):
        self.items = None

    def histogramData(self, reqId, items):
        self.items = items


def _bucket(price, size):
    b = ibx.HistogramData()
    b.price = price
    b.size = size
    return b


def test_this_engines_record_crosses_as_theirs():
    caught = Caught()
    bridge = _LoopBound(caught)
    bridge.histogram_data(7, [_bucket(101.5, 42), _bucket(102.0, 7)])

    assert caught.items is not None, "the histogram reached their wrapper"
    # Read exactly as their wrapper reads it.
    assert [(i.price, i.count) for i in caught.items] == [(101.5, 42), (102.0, 7)]


def test_a_plain_pair_still_crosses():
    """The shape the bridge was written against still works."""
    caught = Caught()
    bridge = _LoopBound(caught)
    bridge.histogram_data(7, [(101.5, 42)])
    assert [(i.price, i.count) for i in caught.items] == [(101.5, 42)]
