"""A request number already watching a contract is refused, under 102.

The number a caller gives a market data request is the only thing the ticks
come back under. Given a second contract, the two records that answer "who is
watching this contract" and "what is this request watching" disagreed: the
second contract took the request, the first stayed in the one the delivery loop
reads, and the caller was handed both contracts' ticks under one number with
nothing to tell them apart.

Refused instead, on the error callback, under the number that names it — which
is what a program branches on.
"""

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code, msg))


def contract(con_id, symbol):
    c = ibx.Contract()
    c.conId = con_id
    c.symbol = symbol
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def test_a_second_contract_under_a_live_number_is_refused():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_instrument(5, 7)

    c.reqMktData(5, contract(320227571, "QQQ"), "", False, False, [])

    assert [(r, code) for r, code, _ in w.seen] == [(5, 102)], (
        f"the number is already watching something: {w.seen}"
    )
