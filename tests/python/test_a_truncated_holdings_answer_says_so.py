"""An answer given before the account finished stating its holdings says so.

reqPositions waits for the venue to finish stating what the account holds and
then answers. An account that says nothing within the wait is answered with
what this session already had, which reads exactly like an account holding
nothing — so the wait running out has to reach the caller, not only the log.

The Rust surface reported it and the Python surface did not, so whether a
caller could tell a truncated answer from a complete one depended on which
language they wrote in.

Run: pytest tests/python/test_a_truncated_holdings_answer_says_so.py -v
"""

from ibx import EWrapper, EClient


class Recorder(EWrapper):
    def __init__(self):
        self.errors = []
        self.ended = 0

    def error(self, req_id, error_time, code, message, advanced_order_reject_json=""):
        self.errors.append((req_id, message))

    def position_end(self):
        self.ended += 1


def test_an_answer_given_before_the_account_finished_says_so():
    w = Recorder()
    c = EClient(w)
    c._test_connect()

    # No venue behind this session, so the account never finishes stating.
    c.req_positions()
    c._test_dispatch_once()

    assert w.ended == 1, "the answer is still given"
    assert any("had not finished stating its holdings" in why for _, why in w.errors), w.errors
    assert all(req_id == -1 for req_id, _ in w.errors), "under the id no request has"
