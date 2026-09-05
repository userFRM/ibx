"""The profit reported is this session's account's, whatever account is named.

The figures are worked out from one set of midnight seeds against one book of
holdings, and both belong to the account this session opened under. Taken at
its word, a subscription for another account replaced those seeds with that
account's and the next restatement replaced them back -- so the figure reported
under one request alternated between two accounts' realised legs measured
against a third thing, this account's positions. A wrong money figure, with
nothing said.
"""

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code, msg))


def test_naming_another_account_is_answered_rather_than_taken():
    w = Errors()
    c = ibx.EClient(w)
    c._test_connect("DU123")

    c.reqPnL(9, "DU999", "")

    told = [msg for _, _, msg in w.seen if "DU999" in msg]
    assert told, f"the caller is told which account the profit is: {w.seen}"
    assert "DU123" in told[0], f"and which account it belongs to: {told[0]}"

    sent = c._test_take_commands()
    assert any("DU123" in cmd for cmd in sent), (
        f"this account is what the venue is asked for: {sent}"
    )
    assert not any("DU999" in cmd for cmd in sent), (
        f"and the named one is not: {sent}"
    )
