"""One contract, one subscription on the wire, every caller told the same thing.

Several requests can watch the same contract, and the quote is delivered under
each caller's own request id. The moment of the last trade was delivered under
one of them only, so a second caller had every tick except the one that says
whether the print it is holding happened now or hours ago.
"""

import ibx


class Ticks(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.strings = []

    def tickString(self, reqId, tickType, value):
        self.strings.append((reqId, tickType, value))

    def error(self, *a):
        pass


def test_the_last_trade_time_reaches_the_followers_too():
    w = Ticks()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_instrument(1, 7)
    c._test_follow_instrument(2, 7)
    c._test_push_quote(7, 412.0, 412.1, 412.05, 10, 10, 1, 1000, 0.0, 0.0, 0.0, 0.0)
    c._test_dispatch_once()

    stamped = {req_id for req_id, kind, _ in w.strings
               if kind == ibx.TickTypeEnum.LAST_TIMESTAMP}
    assert stamped == {1, 2}, f"the follower was not told when it traded: {w.strings}"


class Everything(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.greeks = []
        self.news = []

    def tickOptionComputation(self, reqId, tickType, tickAttrib, impliedVol, delta,
                              optPrice, pvDividend, gamma, vega, theta, undPrice):
        self.greeks.append(reqId)

    def tickNews(self, tickerId, timeStamp, providerCode, articleId, headline, extraData):
        self.news.append(tickerId)

    def error(self, *a):
        pass


def test_the_venue_s_option_model_reaches_the_followers_too():
    """The model belongs to the contract, not to whoever asked first. Sent to
    the owner alone, a second caller on the same option had every tick but its
    Greeks and nothing said they were missing."""
    w = Everything()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_instrument(1, 7)
    c._test_follow_instrument(2, 7)
    c._test_push_option_model(7, 0.21, 4.35, 412.0)
    c._test_dispatch_once()
    assert set(w.greeks) == {1, 2}, f"the follower was not told the model: {w.greeks}"


def test_news_about_a_contract_reaches_the_followers_too():
    w = Everything()
    c = ibx.EClient(w)
    c._test_connect("T")
    c._test_map_instrument(1, 7)
    c._test_follow_instrument(2, 7)
    c._test_push_tick_news(7, "BRFG", "a1", "a headline")
    c._test_dispatch_once()
    assert set(w.news) == {1, 2}, f"the follower heard no news: {w.news}"
