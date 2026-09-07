"""A callback hands over the records the reference client states, not bare rows.

Four callbacks handed the caller tuples (or, for the smart components, a dict
keyed by bit number) where the reference client passes objects. Code written
against that client reads a field by name — `item.price`, `code.accountID` — and
a row answers to none of them. The attribute error is caught by the dispatcher
that called the handler, so the caller was handed a result it could not read and
heard nothing said about it either. The schedule was worse than unreadable: its
triple ran in this client's own order, so a program reading it positionally took
the reference date for the opening time.
"""

import ibx


class Records(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.smart = None
        self.family = None
        self.histogram = None
        self.sessions = None

    def smartComponents(self, reqId, smartComponentMap):
        self.smart = smartComponentMap

    def familyCodes(self, familyCodes):
        self.family = familyCodes

    def histogramData(self, reqId, items):
        self.histogram = items

    def historicalSchedule(self, reqId, startDateTime, endDateTime, timeZone, sessions):
        self.sessions = sessions


def _client():
    w = Records()
    c = ibx.EClient(w)
    c._test_connect("T")
    return w, c


def test_smart_components_arrive_as_a_list_of_records():
    """The reference decoder builds a list; the name it gives it says "map"."""
    w, c = _client()
    c._test_note_reference_data(3, "ISLAND", "I", "tier", "val", "brand")
    c.reqSmartComponents(9, "a")
    c.poll()
    assert isinstance(w.smart, list), f"a dict is not what the reference passes: {type(w.smart)}"
    assert w.smart[0].bitNumber == 3
    assert w.smart[0].exchange == "ISLAND"
    assert w.smart[0].exchangeLetter == "I"


def test_family_codes_arrive_as_records():
    w, c = _client()
    c._test_set_family_codes("DU123", "Fam")
    c.reqFamilyCodes()
    c.poll()
    assert w.family, "the codes reached nothing"
    assert w.family[0].accountID == "DU123"
    assert w.family[0].familyCodeStr == "Fam"


def test_a_histogram_bucket_names_its_price_and_size():
    w, c = _client()
    c._test_push_histogram(7, 101.5, 42)
    c.poll()
    assert w.histogram, "the histogram reached nothing"
    assert w.histogram[0].price == 101.5
    assert w.histogram[0].size == 42


def test_a_schedule_session_names_its_times_the_way_the_reference_does():
    """And carries them under the right names — the triple ran the other way."""
    w, c = _client()
    c._test_push_historical_schedule(11, "20260907", "09:30:00", "16:00:00")
    c.poll()
    assert w.sessions, "the schedule reached nothing"
    s = w.sessions[0]
    assert s.startDateTime == "09:30:00", "the opening time, not the reference date"
    assert s.endDateTime == "16:00:00"
    assert s.refDate == "20260907"
