"""A callback reaches whichever spelling the caller overrode.

`super()` reads each class's own contents and asks no instance hook, so the
reference client's spelling has to be a name on the class for
`super().tickPrice(...)` to find anything — and a program written against that
client calls `super()` first in most of its overrides.

Put there as the method under a second name, the base then stands in front of a
subclass that overrode only this client's spelling, and that subclass stops
being reached at all. So the base answers with a function, which a bound form
carries `__func__` for, and delivery passes the base's own answer over.
"""

import ibx


class Traced(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.ran = []

    def error(self, *a):
        pass


class SnakeOnly(Traced):
    def tick_price(self, *a):
        self.ran.append("tick_price")


class CamelOnly(Traced):
    def tickPrice(self, req_id, tick_type, price, attrib):
        super().tickPrice(req_id, tick_type, price, attrib)
        self.ran.append("tickPrice")


class Both(Traced):
    def tick_price(self, *a):
        self.ran.append("tick_price")

    def tickPrice(self, *a):
        self.ran.append("tickPrice")


class Neither(Traced):
    pass


def _delivered_to(wrapper):
    client = ibx.EClient(wrapper)
    client._test_connect("T")
    client._test_map_instrument(1, 7)
    client._test_push_quote(7, bid=412.0, bid_size=100)
    client._test_dispatch_once()
    return wrapper.ran


def test_a_subclass_that_overrode_this_clients_spelling_is_reached():
    assert _delivered_to(SnakeOnly()) == ["tick_price"]


def test_a_subclass_that_overrode_the_reference_spelling_is_reached():
    assert _delivered_to(CamelOnly()) == ["tickPrice"]


def test_a_subclass_that_overrode_both_is_reached_under_the_reference_one():
    assert _delivered_to(Both()) == ["tickPrice"]


def test_a_subclass_that_overrode_neither_hears_nothing_and_raises_nothing():
    assert _delivered_to(Neither()) == []


def test_super_reaches_the_base_from_a_reference_spelled_override():
    """And not the sibling: a version resolving on the instance would recurse."""
    wrapper = CamelOnly()
    wrapper.tickPrice(1, 4, 100.0, ibx.TickAttrib())
    assert wrapper.ran == ["tickPrice"]
