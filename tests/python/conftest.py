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
