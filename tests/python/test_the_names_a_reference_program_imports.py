"""A program written against the reference client imports these on line one.

Its samples fill in a plain object and hand it back — an execution filter, a
scanner subscription, a combination leg — name the constant an unset field
carries, and annotate every callback with an alias. All of that is evaluated
before the program does anything, so a name absent here is an ImportError or a
NameError at class-definition time and nothing runs at all.

These are read by attribute on the way to the venue, so the shape is the whole
contract: an object of any type carrying the same attribute names is already
accepted, and these are what a caller reaches for when they have no reason to
write their own.
"""

import ibx


def test_the_objects_a_caller_fills_in_exist_and_are_read():
    execution_filter = ibx.ExecutionFilter()
    execution_filter.acctCode = "DU1"
    execution_filter.side = "BUY"
    assert execution_filter.clientId == 0
    assert execution_filter.symbol == ""

    scan = ibx.ScannerSubscription()
    scan.instrument = "STK"
    scan.locationCode = "STK.US.MAJOR"
    scan.scanCode = "TOP_PERC_GAIN"
    # An unset bound is the largest number there is, which is what tells this
    # client not to send it. Sent, it would empty the scan rather than widen it.
    assert scan.abovePrice == ibx.UNSET_DOUBLE
    assert scan.aboveVolume == ibx.UNSET_INTEGER
    assert scan.numberOfRows == -1

    leg = ibx.ComboLeg()
    leg.conId = 265598
    leg.ratio = 1
    leg.action = "BUY"
    leg.exchange = "SMART"
    assert leg.exemptCode == -1, "the reference leaves it at minus one, not nought"

    hedge = ibx.DeltaNeutralContract()
    hedge.conId = 756733
    hedge.delta = 0.5
    hedge.price = 100.0

    ibx.OrderComboLeg()
    ibx.OrderCancel()
    ibx.WshEventData()


def test_a_filter_a_caller_filled_in_reaches_the_request():
    # The point of the object: this client reads it by attribute, so what the
    # caller set is what the request asks for.
    client = ibx.EClient(ibx.EWrapper())
    execution_filter = ibx.ExecutionFilter()
    execution_filter.symbol = "SPY"
    execution_filter.side = "BUY"
    client.reqExecutions(1, execution_filter)

    scan = ibx.ScannerSubscription()
    scan.instrument = "STK"
    scan.locationCode = "STK.US.MAJOR"
    scan.scanCode = "TOP_PERC_GAIN"
    client.reqScannerSubscription(2, scan, [], [])


def test_the_constants_an_unset_field_carries():
    assert ibx.UNSET_INTEGER == 2**31 - 1
    assert ibx.UNSET_LONG == 2**63 - 1
    assert ibx.UNSET_DOUBLE == __import__("sys").float_info.max
    assert str(ibx.UNSET_DECIMAL) == str(2**127 - 1)
    assert ibx.DOUBLE_INFINITY == float("inf")
    assert ibx.INFINITY_STR == "Infinity"
    assert ibx.NO_VALID_ID == -1
    assert ibx.MAX_MSG_LEN == 0xFFFFFF


def test_the_aliases_a_callback_annotation_names():
    # Evaluated when the class body runs, so a program with annotated
    # overrides — which the reference's own sample has — needs them present
    # before it has done anything.
    assert (ibx.TickerId, ibx.OrderId, ibx.TickType) == (int, int, int)
    assert ibx.TagValueList is list
    assert ibx.SetOfString is set and ibx.SetOfFloat is set
    assert ibx.SmartComponentMap is dict
    assert ibx.ListOfContractDescription is list
    assert ibx.HistogramDataList is list


def test_the_marker_every_override_carries():
    # It marks and does nothing else; a program will not import without it.
    @ibx.iswrapper
    def answered(a, b):
        return a + b

    assert answered(1, 2) == 3


def test_a_figure_is_written_for_a_person_and_an_unset_one_is_not():
    assert ibx.intMaxString(7) == "7"
    assert ibx.intMaxString(ibx.UNSET_INTEGER) == ""
    assert ibx.floatMaxString(1.5) == "1.5"
    assert ibx.floatMaxString(ibx.UNSET_DOUBLE) == ""
    assert ibx.longMaxString(ibx.UNSET_LONG) == ""
    assert ibx.decimalMaxString(ibx.UNSET_DECIMAL) == ""


def test_the_account_figures_are_named():
    tags = ibx.AccountSummaryTags
    assert tags.NetLiquidation == "NetLiquidation"
    assert tags.SMA == "SMA"
    named = tags.AllTags.split(",")
    assert "NetLiquidation" in named and "DayTradesRemaining" in named
    assert len(named) == len(set(named)), "each figure named once"
