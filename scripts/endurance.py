"""Hold a session open under load, and fail if anything stops arriving.

What only a session finds: a connection that stops answering, a stream that
stops arriving, a request id that answers once. None of it is reachable by a
suite that opens a session, asks one question and closes it — every defect
this has found was invisible to the offline gates and to the live suites
alike.

Subscriptions are taken out and withdrawn every cycle, under request ids that
repeat, because that is what a program does and it is where the defects were.
Every stream is required to have grown by the end of every cycle after the
first: a stream that stalls is the failure, and a run that only printed its
counters would have passed through all of them.

Places no orders. Reads only.

    IB_USERNAME=… IB_PASSWORD=… python scripts/endurance.py --minutes 20
"""

import argparse
import collections
import os
import sys
import threading
import time

from ibx import Contract, EClient, EWrapper

#: Liquid enough to be quoting whenever a market is open, and spread across two
#: asset classes so one venue going quiet does not read as the client stalling.
SUBJECTS = [
    ("SPY", "STK", "SMART"),
    ("TSLA", "STK", "SMART"),
    ("AAPL", "STK", "SMART"),
    ("QQQ", "STK", "SMART"),
    ("EUR", "CASH", "IDEALPRO"),
]

#: What the venue says when a currency pair is asked for trades it does not
#: have. Expected once a cycle, and not a defect.
NO_TRADES_FOR_A_CURRENCY_PAIR = 162

#: Notices about a connection coming and going, which are not errors.
CONNECTION_NOTICES = {2100, 2103, 2104, 2105, 2106, 2107, 2119, 2158}


class Counters(EWrapper):
    """Everything that arrived, by kind."""

    def __init__(self):
        super().__init__()
        self.ready = threading.Event()
        self.lock = threading.Lock()
        self.seen = collections.Counter()
        self.errors = collections.Counter()
        self.last_error = None

    def next_valid_id(self, order_id):
        self.ready.set()

    def managed_accounts(self, accounts):
        pass

    def connect_ack(self):
        pass

    def tick_price(self, *rest):
        self._note("quotes")

    def tick_size(self, *rest):
        self._note("quotes")

    def update_mkt_depth_l2(self, *rest):
        self._note("book")

    def update_mkt_depth(self, *rest):
        self._note("book")

    def tick_by_tick_all_last(self, *rest):
        self._note("trades")

    def historical_data(self, *rest):
        self._note("bars")

    def error(self, req_id, code, message, advanced=""):
        if code in CONNECTION_NOTICES:
            return
        with self.lock:
            self.errors[code] += 1
            self.last_error = (req_id, code, message[:100])

    def _note(self, kind):
        with self.lock:
            self.seen[kind] += 1

    def snapshot(self):
        with self.lock:
            return dict(self.seen), dict(self.errors), self.last_error


#: What every market owes a session that asked for it. A book and a trade
#: stream are the venue's to grant, so they are held to arriving at all rather
#: than to arriving in every cycle.
REQUIRED_EVERY_CYCLE = ("quotes", "bars")


def what_stopped(before, now, cycle):
    """Which streams stopped arriving between one cycle and the next.

    A counter that has not moved is a stream that stopped. That is the whole
    finding: bars stopped after the seventh minute while every other stream
    stayed healthy, and nothing but a session watching its own counters would
    have said so.
    """
    return [
        f"{kind} stopped arriving after cycle {cycle - 1}"
        for kind in REQUIRED_EVERY_CYCLE
        if now.get(kind, 0) <= before.get(kind, 0)
    ]


def contract(symbol, sec_type, exchange):
    made = Contract()
    made.symbol, made.sec_type, made.exchange = symbol, sec_type, exchange
    made.currency = "USD"
    return made


def main():
    parsed = argparse.ArgumentParser(description=__doc__)
    parsed.add_argument("--minutes", type=int, default=20)
    parsed.add_argument("--host", default=os.environ.get("IB_HOST", "cdc1.ibllc.com"))
    asked = parsed.parse_args()

    username = os.environ.get("IB_USERNAME", "")
    password = os.environ.get("IB_PASSWORD", "")
    if not username or not password:
        print("IB_USERNAME and IB_PASSWORD are unset; this needs a session.")
        return 2

    watcher = Counters()
    client = EClient(watcher)
    client.connect(username=username, password=password, host=asked.host, paper=True)
    threading.Thread(target=client.run, daemon=True).start()
    if not watcher.ready.wait(timeout=60):
        print("no session was opened within a minute")
        return 1

    started = time.time()
    cycle = 0
    before = {}
    stalled = []
    while time.time() - started < asked.minutes * 60:
        cycle += 1
        # Ids repeat every seventh cycle, which is what a program does and is
        # where a request id that answers only once shows itself.
        base = 1000 + (cycle % 7) * 100
        for n, (symbol, sec_type, exchange) in enumerate(SUBJECTS):
            what = contract(symbol, sec_type, exchange)
            client.req_mkt_data(base + n, what, "", False, False)
            if sec_type == "STK":
                client.req_mkt_depth(base + 50 + n, what, 10, True)
                client.req_tick_by_tick_data(base + 70 + n, what, "AllLast", 0, False)
            client.req_historical_data(
                base + 90 + n, what, "", "1 D", "1 hour", "TRADES", 1, 1, False, [],
            )
        time.sleep(60)
        for n, (symbol, sec_type, _) in enumerate(SUBJECTS):
            client.cancel_mkt_data(base + n)
            if sec_type == "STK":
                client.cancel_mkt_depth(base + 50 + n, True)
                client.cancel_tick_by_tick_data(base + 70 + n)
        time.sleep(5)

        seen, errors, last = watcher.snapshot()
        minutes = (time.time() - started) / 60
        print(f"[{minutes:5.1f}m] cycle {cycle:3d} {seen} errors={errors} last={last}",
              flush=True)

        if cycle > 1:
            stalled.extend(what_stopped(before, seen, cycle))
        before = seen

    unexpected = {code: n for code, n in watcher.snapshot()[1].items()
                  if code != NO_TRADES_FOR_A_CURRENCY_PAIR}
    client.disconnect()

    for what in stalled:
        print(f"STALLED: {what}")
    if unexpected:
        print(f"ERRORS: {unexpected}")
    if stalled or unexpected:
        return 1
    print(f"held open for {asked.minutes} minutes with nothing stopping")
    return 0


if __name__ == "__main__":
    sys.exit(main())
