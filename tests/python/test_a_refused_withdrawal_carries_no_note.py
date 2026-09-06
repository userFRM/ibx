"""The note that a withdrawal's time does not travel is said for a withdrawal that happens.

A withdrawal refused as naming no working order withdrew nothing, and the note
that its time would not travel was said all the same, before the refusal.

Run: pytest tests/python/test_a_refused_withdrawal_carries_no_note.py -v
"""

from ibx import EWrapper, EClient


class Recorder(EWrapper):
    def __init__(self):
        self.errors = []

    def error(self, req_id, error_time, code, message, advanced_order_reject_json=""):
        self.errors.append((req_id, code, message))


class Cancel:
    def __init__(self):
        self.manualOrderCancelTime = "20260906-10:00:00"
        self.extOperator = ""
        self.manualOrderIndicator = 0


def test_a_refused_withdrawal_carries_no_note():
    w = Recorder()
    c = EClient(w)
    c._test_connect()
    c.cancel_order(77, Cancel())
    said = [m for req_id, _, m in w.errors if req_id == 77]
    assert len(said) == 1, said
    assert "no order is working" in said[0], said
