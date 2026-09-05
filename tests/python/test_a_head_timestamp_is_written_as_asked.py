"""A head timestamp is written the way the caller asked, as the other surface
writes it. Handed back as the venue wrote it, a caller asking for seconds since
the epoch read a date string."""
import ibx


class Head(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.seen = []

    def headTimestamp(self, reqId, headTimestamp):
        self.seen.append((reqId, headTimestamp))

    def error(self, *a):
        pass


def spy():
    c = ibx.Contract()
    c.conId = 756733
    c.symbol = "SPY"
    c.secType = "STK"
    c.exchange = "SMART"
    c.currency = "USD"
    return c


def test_a_head_timestamp_is_written_as_asked():
    w = Head()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_con_id(756733, 0)
    c.reqHeadTimeStamp(7, spy(), "TRADES", 1, 2)
    c._test_push_head_timestamp(7, "20200101-00:00:00")
    c._test_dispatch_once()
    assert w.seen and int(w.seen[0][1]) == 1577836800, w.seen
