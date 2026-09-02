"""The names a program written against the reference client imports.

That client publishes a handful of plain objects a caller fills in and hands
back — an execution filter, a scanner subscription, a combination leg — along
with the constants its unset fields carry and the aliases its type annotations
name. A program imports them on its first line, so absent, none of it ran.

Everything here is plain Python, as it is there, because this client reads
these objects by attribute: the shape is the contract, and any object carrying
the same attribute names is already accepted. What each field means is the
venue's business and is documented by the venue; what is written here is the
shape and nothing else.
"""

import math
import sys
from decimal import Decimal

#: An integer field nobody set.
UNSET_INTEGER = 2**31 - 1
#: A floating-point field nobody set.
UNSET_DOUBLE = float(sys.float_info.max)
#: A long field nobody set.
UNSET_LONG = 2**63 - 1
#: A decimal field nobody set.
UNSET_DECIMAL = Decimal(2**127 - 1)
#: A price with no bound above it.
DOUBLE_INFINITY = math.inf
#: How that price is written on the wire.
INFINITY_STR = "Infinity"
#: The request id an unsolicited answer carries.
NO_VALID_ID = -1
#: The largest message the protocol carries, one byte under sixteen megabytes.
MAX_MSG_LEN = 0xFFFFFF

# Aliases the reference client's annotations name. A program evaluates them at
# class-definition time — its callbacks are annotated with them — so a missing
# one is a NameError before the class exists, not a typing nicety.
TickerId = int
OrderId = int
TickType = int
TagValueList = list
SetOfString = set
SetOfFloat = set
SmartComponentMap = dict
ListOfContractDescription = list
ListOfDepthExchanges = list
ListOfNewsProviders = list
ListOfPriceIncrements = list
ListOfFamilyCode = list
ListOfHistoricalTick = list
ListOfHistoricalTickBidAsk = list
ListOfHistoricalTickLast = list
ListOfHistoricalSessions = list
HistogramDataList = list


def iswrapper(fn):
    """Mark an override as answering a callback.

    A marker and nothing else, as it is on the reference client: its samples
    put it on every override, so a program will not import without it, and it
    changes nothing about the function it is given.
    """
    return fn


class ExecutionFilter:
    """Which of the day's executions a request is asking for.

    Left as it is, it asks for all of them.
    """

    def __init__(self):
        self.clientId = 0
        self.acctCode = ""
        self.time = ""
        self.symbol = ""
        self.secType = ""
        self.exchange = ""
        self.side = ""


class ScannerSubscription:
    """What a scan is looking for.

    `instrument`, `locationCode` and `scanCode` name the scan; the rest bound
    it. A numeric bound left at its unset value is not sent, because a bound of
    the largest number there is would empty the scan rather than widen it.
    """

    def __init__(self):
        self.numberOfRows = -1
        self.instrument = ""
        self.locationCode = ""
        self.scanCode = ""
        self.abovePrice = UNSET_DOUBLE
        self.belowPrice = UNSET_DOUBLE
        self.aboveVolume = UNSET_INTEGER
        self.marketCapAbove = UNSET_DOUBLE
        self.marketCapBelow = UNSET_DOUBLE
        self.moodyRatingAbove = ""
        self.moodyRatingBelow = ""
        self.spRatingAbove = ""
        self.spRatingBelow = ""
        self.maturityDateAbove = ""
        self.maturityDateBelow = ""
        self.couponRateAbove = UNSET_DOUBLE
        self.couponRateBelow = UNSET_DOUBLE
        self.excludeConvertible = False
        self.averageOptionVolumeAbove = UNSET_INTEGER
        self.scannerSettingPairs = ""
        self.stockTypeFilter = ""


class ComboLeg:
    """One leg of a combination contract.

    A leg with no `conId` names no contract and is refused; a leg with no
    `ratio` is a leg the caller had not finished describing, and is not made
    into a one-for-one leg on their behalf.
    """

    SAME = 0
    OPEN = 1
    CLOSE = 2
    UNKNOWN = 3

    def __init__(self):
        self.conId = 0
        self.ratio = 0
        self.action = ""
        self.exchange = ""
        self.openClose = 0
        self.shortSaleSlot = 0
        self.designatedLocation = ""
        self.exemptCode = -1


class DeltaNeutralContract:
    """The contract a delta-neutral order hedges against, and at what."""

    def __init__(self):
        self.conId = 0
        self.delta = 0.0
        self.price = 0.0


class OrderComboLeg:
    """The price a combination order names for one of its legs."""

    def __init__(self, price=UNSET_DOUBLE):
        self.price = price


class OrderCancel:
    """What a withdrawal states beyond the order it withdraws."""

    def __init__(self):
        self.manualOrderCancelTime = ""
        self.extOperator = ""
        self.manualOrderIndicator = UNSET_INTEGER


class WshEventData:
    """Which corporate events a request is asking for."""

    def __init__(self):
        self.conId = UNSET_INTEGER
        self.filter = ""
        self.fillWatchlist = False
        self.fillPortfolio = False
        self.fillCompetitors = False
        self.startDate = ""
        self.endDate = ""
        self.totalLimit = UNSET_INTEGER


class AccountSummaryTags:
    """The account figures a summary can be asked for, by name."""

    AccountType = "AccountType"
    NetLiquidation = "NetLiquidation"
    TotalCashValue = "TotalCashValue"
    SettledCash = "SettledCash"
    AccruedCash = "AccruedCash"
    BuyingPower = "BuyingPower"
    EquityWithLoanValue = "EquityWithLoanValue"
    PreviousDayEquityWithLoanValue = "PreviousDayEquityWithLoanValue"
    GrossPositionValue = "GrossPositionValue"
    ReqTEquity = "ReqTEquity"
    ReqTMargin = "ReqTMargin"
    SMA = "SMA"
    InitMarginReq = "InitMarginReq"
    MaintMarginReq = "MaintMarginReq"
    AvailableFunds = "AvailableFunds"
    ExcessLiquidity = "ExcessLiquidity"
    Cushion = "Cushion"
    FullInitMarginReq = "FullInitMarginReq"
    FullMaintMarginReq = "FullMaintMarginReq"
    FullAvailableFunds = "FullAvailableFunds"
    FullExcessLiquidity = "FullExcessLiquidity"
    LookAheadNextChange = "LookAheadNextChange"
    LookAheadInitMarginReq = "LookAheadInitMarginReq"
    LookAheadMaintMarginReq = "LookAheadMaintMarginReq"
    LookAheadAvailableFunds = "LookAheadAvailableFunds"
    LookAheadExcessLiquidity = "LookAheadExcessLiquidity"
    HighestSeverity = "HighestSeverity"
    DayTradesRemaining = "DayTradesRemaining"
    Leverage = "Leverage"

    AllTags = ",".join(
        value for name, value in sorted(vars().items())
        if not name.startswith("_") and isinstance(value, str)
    )


def floatMaxString(value):
    """A float written for a person, and nothing where nobody set one."""
    if value is None or value == UNSET_DOUBLE:
        return ""
    return str(value)


def intMaxString(value):
    """An integer written for a person, and nothing where nobody set one."""
    if value is None or value == UNSET_INTEGER:
        return ""
    return str(value)


def longMaxString(value):
    """A long written for a person, and nothing where nobody set one."""
    if value is None or value == UNSET_LONG:
        return ""
    return str(value)


def decimalMaxString(value):
    """A quantity written for a person, and nothing where nobody set one."""
    if value is None or value == UNSET_DECIMAL:
        return ""
    return str(value)

class Object:
    """The base the reference client's plain objects are written on.

    A program subclasses it for objects of its own — its own sample does — so
    it has to exist before that class body runs. It adds nothing.
    """

    def __str__(self):
        return type(self).__name__

    def __repr__(self):
        return f"{id(self)}: {self}"


class ScanData:
    """One row of a scan, as a callback hands it over."""

    def __init__(self, contract=None, rank=0, distance="", benchmark="",
                 projection="", legsStr=""):
        self.contract = contract
        self.rank = rank
        self.distance = distance
        self.benchmark = benchmark
        self.projection = projection
        self.legsStr = legsStr


class OrderCondition:
    """What kind of thing an order's condition watches.

    The numbers the venue gives each kind, which a program compares against
    `condType` and passes when it builds one.
    """

    Price = 1
    Time = 3
    Margin = 4
    Execution = 5
    Volume = 6
    PercentChange = 7


class MarketDataTypeEnum:
    """Which feed a subscription asks for, by the venue's numbering."""

    REALTIME = 1
    FROZEN = 2
    DELAYED = 3
    DELAYED_FROZEN = 4


class FaDataTypeEnum:
    """Which advisor document a request is asking for."""

    GROUPS = 1
    ALIASES = 3


#: A mid-offset that means "up to the midpoint" rather than a distance.
COMPETE_AGAINST_BEST_OFFSET_UP_TO_MID = DOUBLE_INFINITY


def getTimeStrFromMillis(time):
    """A millisecond clock reading written for a person, and nothing for none."""
    if not time or time <= 0:
        return ""
    import datetime

    stamp = datetime.datetime.fromtimestamp(time / 1000.0)
    return stamp.strftime("%b %d, %Y %H:%M:%S.%f")[:-3]


def getEnumTypeName(cls, value):
    """The name a numbered kind goes by, or the first one where it names none."""
    for name, held in vars(cls).items():
        if not name.startswith("_") and held == value:
            return name
    named = [n for n in vars(cls) if not n.startswith("_")]
    return named[0] if named else ""

