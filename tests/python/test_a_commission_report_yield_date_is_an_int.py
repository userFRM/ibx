"""A commission report's yield redemption date is an int in YYYYMMDD form, as
the reference client holds it — held as a string, `report.yieldRedemptionDate >
20260101` raised.

Run: pytest tests/python/test_a_commission_report_yield_date_is_an_int.py -v
"""

from ibx import CommissionAndFeesReport


def test_the_yield_redemption_date_is_an_int_defaulting_to_zero():
    r = CommissionAndFeesReport()
    assert r.yieldRedemptionDate == 0
    assert isinstance(r.yieldRedemptionDate, int)
    r.yieldRedemptionDate = 20300615
    assert r.yieldRedemptionDate == 20300615
