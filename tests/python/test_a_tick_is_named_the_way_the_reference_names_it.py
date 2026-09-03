"""A program reads a tick back by its name, off the client's own table.

The reference client builds that table by enumerating one list of names, so
the position in the list is the tick type a callback is handed, and its
`TickTypeEnum.toStr(tickType)` turns the number back into the name. Its own
synchronous layer keys everything it collects on exactly that call — a quote
arrives and is filed under `"BID"` — so a table short of a name, or without
the call, is an AttributeError in the middle of a callback rather than a
missing constant somewhere harmless.

Which is how this was found: driving that layer against a live crypto pair,
where every tick raised `type object 'TickTypeEnum' has no attribute 'toStr'`.
"""

import ibx

T = ibx.TickTypeEnum


def test_the_number_a_name_carries_is_its_place_in_the_table():
    # The first ten, which every quote is made of.
    assert (T.BID_SIZE, T.BID, T.ASK, T.ASK_SIZE) == (0, 1, 2, 3)
    assert (T.LAST, T.LAST_SIZE, T.HIGH, T.LOW) == (4, 5, 6, 7)
    assert (T.VOLUME, T.CLOSE) == (8, 9)
    # And further in, where a subset ran out.
    assert T.OPEN == 14
    assert T.LAST_TIMESTAMP == 45
    assert T.HALTED == 49
    assert T.NOT_SET == 111


def test_the_name_is_the_reference_spelling_and_not_a_longer_one():
    # Written out by hand, three of these were given the long spelling, which
    # is not the name a program written against that client asks for.
    assert (T.BID_EXCH, T.ASK_EXCH, T.LAST_EXCH) == (32, 33, 84)
    for invented in ("BID_EXCHANGE", "ASK_EXCHANGE", "LAST_EXCHANGE"):
        assert not hasattr(T, invented), f"{invented} is not a name that client has"


def test_every_tick_the_venue_numbers_has_a_name():
    named = [name for name in dir(T) if name.isupper()]
    assert len(named) == 112, f"the table is {len(named)} long"
    # Each one numbered its own place, none twice.
    numbers = sorted(getattr(T, name) for name in named)
    assert numbers == list(range(112))


def test_a_number_reads_back_as_the_name_it_was_filed_under():
    assert T.toStr(T.BID) == "BID"
    assert T.toStr(T.LAST_TIMESTAMP) == "LAST_TIMESTAMP"
    assert T.toStr(T.BID_EXCH) == "BID_EXCH"
    # Every name in the table round-trips.
    for name in (n for n in dir(T) if n.isupper()):
        assert T.toStr(getattr(T, name)) == name


def test_a_number_the_table_does_not_name():
    # Said the way that client says it, because a program compares against it.
    assert T.toStr(999) == "NOTFOUND"
    assert T.toStr(-1) == "NOTFOUND"
    assert T.toStr(112) == "NOTFOUND"


def test_the_table_is_readable_as_a_mapping_too():
    assert T.idx2name[1] == "BID"
    assert len(T.idx2name) == 112
