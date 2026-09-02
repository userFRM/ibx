"""Two callers on the facade at once get their own request ids.

The wheel is built free-threaded, so two callers really do run here at once.
The counter read and wrote without a guard, so both saw the same number: thirty
thousand collisions in forty thousand calls, measured. Two subscriptions under
one number overwrite each other, so the cancel for the first stops the second
and leaves the first running at the venue.
"""

import threading

from ibx import IB


def test_no_two_callers_are_handed_the_same_request_id():
    ib = IB()
    handed: list[int] = []
    guard = threading.Lock()
    ready = threading.Barrier(8)

    def take():
        ready.wait()
        mine = [ib._next_req_id() for _ in range(500)]
        with guard:
            handed.extend(mine)

    threads = [threading.Thread(target=take) for _ in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    assert len(handed) == len(set(handed)), (
        f"{len(handed) - len(set(handed))} of {len(handed)} were handed out twice"
    )


def test_a_registration_keeps_the_contract_it_was_keyed_on():
    """The key is an address, and an address is only that object's while it lives.

    Kept without the contract, the entry outlived it, the next contract was
    built at the same address, and a cancel naming that one stopped the stream
    belonging to the first — and could never stop its own.
    """
    import weakref

    ib = IB()

    class Anything:
        pass

    contract = Anything()
    ib._remember("bars", contract, 7)
    watch = weakref.ref(contract)
    del contract

    assert watch() is not None, (
        "the registration holds the contract, so no later object is built at "
        "its address while the entry stands"
    )
