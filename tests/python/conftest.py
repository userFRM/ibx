"""Shared fixtures for the compatibility tests."""

from ibx import EWrapper


class NotConnectedProbe(EWrapper):
    """Captures what a call reports when it is made before connecting.

    The reference client answers such a call on the error callback and returns
    normally rather than raising, so that is what these tests assert.
    """

    NOT_CONNECTED = 504

    def __init__(self):
        super().__init__()
        self.errors = []

    def error(self, req_id, code, msg, advanced_order_reject_json=""):
        self.errors.append((req_id, code, msg))

    @property
    def not_connected(self):
        return any(code == self.NOT_CONNECTED for _, code, _ in self.errors)


def next_option_expiry(at_least_days=2) -> str:
    """The next Friday a weekly option on a big US name expires, as yyyymmdd.

    Written out as a fixed date, a test stops exercising anything the moment
    that date passes: the contract stops existing, the quote never arrives, and
    the test skips itself as "market closed" for ever after. Computed, it keeps
    naming a contract that exists.
    """
    import datetime

    day = datetime.date.today() + datetime.timedelta(days=at_least_days)
    day += datetime.timedelta(days=(4 - day.weekday()) % 7)   # 4 is Friday
    return day.strftime("%Y%m%d")
