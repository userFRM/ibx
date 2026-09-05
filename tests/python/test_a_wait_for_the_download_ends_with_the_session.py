"""A wait for the account download ends with the session.

Checked at entry alone, a session that ended inside the wait had the caller sit
out the ten seconds and then read the pre-drop book under a refusal for silence,
where the refusal for no connection was three lines away.
"""
import threading
import time

import ibx


class Errors(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def error(self, req_id, error_time, code, msg, advanced_order_reject_json=""):
        self.seen.append((req_id, code))


def _ends_the_session_soon(c):
    def end():
        time.sleep(0.1)
        c._test_end_session()
    threading.Thread(target=end, daemon=True).start()


def test_the_wait_ends_with_the_session():
    for name, ask in (
        ("reqPositions", lambda c: c.reqPositions()),
        ("reqPositionsMulti", lambda c: c.reqPositionsMulti(9, "", "")),
        ("reqAccountUpdatesMulti", lambda c: c.reqAccountUpdatesMulti(9, "", "", True)),
    ):
        w = Errors()
        c = ibx.EClient(w)
        c._test_connect("T")
        _ends_the_session_soon(c)
        started = time.monotonic()
        ask(c)
        assert time.monotonic() - started < 5, f"{name}: the wait ended with the session, not the clock"
        assert any(code == 504 for _, code in w.seen), f"{name}: and the caller was told: {w.seen}"
