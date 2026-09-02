"""Only one of two callers racing to connect gets a session.

The refusal and the flag it reads were two steps, so both callers found the
flag clear and both built an engine. The second replaced the first, which went
on running with a live socket and a second logon the account never asked for —
and the venue bumps the older session when that happens, so a caller racing
itself knocks over its own.

The wheel is built free-threaded, so this is not a theoretical interleaving.
"""

import threading

import pytest
from ibx import EClient, EWrapper


@pytest.mark.parametrize("attempt", range(20))
def test_only_one_of_two_racing_connects_takes_the_session(attempt):
    client = EClient(EWrapper())
    ready = threading.Barrier(2)
    took = []

    def connect():
        ready.wait()
        try:
            client._test_connect("DU111111", True)
            took.append(True)
        except RuntimeError:
            pass

    threads = [threading.Thread(target=connect) for _ in range(2)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert len(took) == 1, (
        f"{len(took)} callers built a session; the second leaves the first "
        f"running with a live socket and a second logon"
    )
