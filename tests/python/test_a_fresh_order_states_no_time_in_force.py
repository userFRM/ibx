"""A fresh order states no time in force, as the reference client's does — it
held "DAY", where that client holds "" and the venue reads an empty tif as DAY.

Run: pytest tests/python/test_a_fresh_order_states_no_time_in_force.py -v
"""

from ibx import Order


def test_a_fresh_order_states_no_tif():
    assert Order().tif == ""
