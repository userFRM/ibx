"""A refused execution request still delivers its end.

The request refused for naming a filter this client does not apply returned
without the end, and a caller waiting on it waited for good. Every other
request on this surface reports its refusal and still ends.

Run: pytest tests/python/test_a_refused_execution_request_still_ends.py -v
"""

from ibx import EWrapper, EClient


class Recorder(EWrapper):
    def __init__(self):
        self.errors = []
        self.ended = []

    def error(self, req_id, error_time, code, message, advanced_order_reject_json=""):
        self.errors.append((req_id, code))

    def exec_details_end(self, req_id):
        self.ended.append(req_id)


class Filter:
    def __init__(self):
        self.client_id = 0
        self.acct_code = ""
        self.time = ""
        self.symbol = ""
        self.sec_type = ""
        self.exchange = ""
        self.side = ""
        self.lastNDays = 3
        self.specificDates = None


def test_a_refused_execution_request_still_ends():
    w = Recorder()
    c = EClient(w)
    c._test_connect()
    c.req_executions(9, Filter())
    c._test_dispatch_once()
    assert any(req_id == 9 for req_id, _ in w.errors), w.errors
    assert w.ended == [9], "the end still comes"
