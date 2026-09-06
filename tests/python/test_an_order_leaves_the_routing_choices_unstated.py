"""An order's routing choices are unstated until a program states them.

The reference client starts `routeMarketableToBbo`, `seekPriceImprovement`
and `usePriceMgmtAlgo` as None and writes them only once stated. Here they
started as False, False and 0, so a program testing them for None to learn
whether it had chosen read a choice it never made. The submit wrote the same
wire either way.

Run: pytest tests/python/test_an_order_leaves_the_routing_choices_unstated.py -v
"""

from ibx import Order

CHOICES = ["routeMarketableToBbo", "seekPriceImprovement", "usePriceMgmtAlgo"]


def test_a_fresh_order_states_none():
    order = Order()
    assert [getattr(order, name) for name in CHOICES] == [None, None, None]


def test_a_stated_choice_reads_back():
    order = Order()
    order.routeMarketableToBbo, order.seekPriceImprovement, order.usePriceMgmtAlgo = True, False, 1
    assert [getattr(order, name) for name in CHOICES] == [True, False, 1]
    order.route_marketable_to_bbo = None
    assert order.routeMarketableToBbo is None
