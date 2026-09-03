"""The reference client's module layout, so its import lines resolve.

A program written against that client does not import one flat name — it
imports `ibapi.client`, `ibapi.wrapper`, `ibapi.contract`, and reaches for
`wrapper.EWrapper` after `from ibapi import wrapper`. Every one of those lines
is on the first page of the program, so a layout that answers none of them
means no program written that way ever reached its first statement, whatever
else was exported.

Each name below is a module holding what that client's module of the same name
holds, of what this one has. Porting is then the one rename a person would
guess at — `ibapi` becomes `ibx` — rather than a rewrite of every import in
the file and a set of hand-written stand-ins beside it.

Nothing here is a second implementation: each is a view of what
:mod:`ibx` already publishes.
"""

import sys
import types


def _module(name: str, contents: dict, doc: str) -> types.ModuleType:
    """One module of the layout, registered so `from ibx.x import y` resolves."""
    full = f"{__package__}.{name}"
    made = types.ModuleType(full, doc)
    for held, value in contents.items():
        setattr(made, held, value)
    made.__all__ = sorted(contents)
    sys.modules[full] = made
    return made


def _creator(surface: dict):
    """`Create(condType)`, which is how a condition is made in that client.

    Its samples build every conditional order this way — an empty condition of
    the kind named, then its fields set — so the six classes being present is
    not enough on its own.
    """
    by_kind = {
        surface["OrderCondition"].Price: surface["PriceCondition"],
        surface["OrderCondition"].Time: surface["TimeCondition"],
        surface["OrderCondition"].Margin: surface["MarginCondition"],
        surface["OrderCondition"].Execution: surface["ExecutionCondition"],
        surface["OrderCondition"].Volume: surface["VolumeCondition"],
        surface["OrderCondition"].PercentChange: surface["PercentChangeCondition"],
    }

    def Create(condType):
        """An empty condition of the kind named, or nothing for a kind there is not."""
        made = by_kind.get(condType)
        return made() if made is not None else None

    return Create


def install(surface: dict) -> dict:
    """Lay the reference client's module names over `surface`.

    `surface` is what :mod:`ibx` publishes. Returns the modules, so the package
    can bind them as attributes of itself — `from ibx import wrapper` reads the
    attribute, `from ibx.wrapper import EWrapper` reads the registration, and a
    program does both.
    """
    def held(*names):
        """The named things, and a complaint if one of them is not there.

        Filtered to what the surface happens to have, a module quietly came out
        short and the import line that wanted the missing name failed at the
        caller instead of here. Two modules were wrong that way and nothing
        said so.
        """
        absent = [n for n in names if n not in surface]
        if absent:
            raise AttributeError(
                f"the layout names {', '.join(absent)}, which this package does "
                f"not publish: add the shape or stop naming it here",
            )
        return {n: surface[n] for n in names}

    layout = {
        "client": (held("EClient"), "The client, under the name that client gives it."),
        "wrapper": (held("EWrapper"), "The callback base, under that client's name."),
        "contract": (
            held("Contract", "ContractDetails", "ContractDescription", "ComboLeg",
                 "DeltaNeutralContract", "DepthMktDataDescription"),
            "A contract and the things that describe one.",
        ),
        "order": (
            held("Order", "OrderComboLeg", "COMPETE_AGAINST_BEST_OFFSET_UP_TO_MID"),
            "An order and what it can carry.",
        ),
        "order_state": (held("OrderState", "OrderAllocation"), "What the venue says about an order."),
        "order_cancel": (held("OrderCancel"), "What a withdrawal states."),
        "order_condition": (
            dict(
                held("OrderCondition", "PriceCondition", "TimeCondition", "MarginCondition",
                     "ExecutionCondition", "VolumeCondition", "PercentChangeCondition"),
                Create=_creator(surface),
            ),
            "What an order can be made to wait for.",
        ),
        "execution": (held("Execution", "ExecutionFilter"), "A fill and which fills are asked for."),
        "commission_and_fees_report": (held("CommissionAndFeesReport"), "What a fill cost."),
        "scanner": (held("ScannerSubscription", "ScanData"), "A scan and one of its rows."),
        "tag_value": (held("TagValue"), "One named value."),
        "ticktype": (held("TickType", "TickTypeEnum"),
                     "What a tick is. The reference client's module holds these two "
                     "and nothing else; what is stated about a tick lives in `common`."),
        "account_summary_tags": (held("AccountSummaryTags"), "The account figures, by name."),
        "softdollartier": (held("SoftDollarTier"), "A soft-dollar tier."),
        "news": (held("NewsProvider"), "A news provider."),
        "object_implem": (held("Object"), "The base that client's plain objects are written on."),
        "common": (
            held("BarData", "RealTimeBar", "HistogramData", "NewsProvider",
                 "DepthMktDataDescription", "SmartComponent", "TickAttrib",
                 "TickAttribBidAsk", "TickAttribLast", "FamilyCode", "PriceIncrement",
                 "HistoricalTick", "HistoricalTickBidAsk", "HistoricalTickLast",
                 "HistoricalSession", "WshEventData", "FaDataTypeEnum", "MarketDataTypeEnum",
                 "TickerId", "OrderId", "TickType", "TagValueList", "SetOfString",
                 "SetOfFloat", "SmartComponentMap", "HistogramDataList",
                 "ListOfContractDescription", "ListOfDepthExchanges", "ListOfNewsProviders",
                 "ListOfPriceIncrements", "ListOfFamilyCode", "ListOfHistoricalTick",
                 "ListOfHistoricalTickBidAsk", "ListOfHistoricalTickLast",
                 "ListOfHistoricalSessions", "ListOfOrder", "UNSET_DOUBLE", "UNSET_INTEGER", "UNSET_LONG",
                 "UNSET_DECIMAL", "DOUBLE_INFINITY", "INFINITY_STR", "NO_VALID_ID",
                 "MAX_MSG_LEN",
                 # Named in that module because it imports them, and a program
                 # star-imports it and uses them unqualified: its own sample
                 # writes `class Activity(Object)` on the strength of exactly
                 # that. A module short of what a star import there brings is a
                 # NameError in the middle of a class body, which is where this
                 # was found.
                 "Object", "floatMaxString", "intMaxString", "decimalMaxString"),
            "The shapes and the constants a callback signature names.",
        ),
        "const": (held("UNSET_DOUBLE", "UNSET_INTEGER", "UNSET_LONG", "UNSET_DECIMAL",
                       "DOUBLE_INFINITY", "INFINITY_STR", "NO_VALID_ID", "MAX_MSG_LEN"),
                  "What an unset field carries."),
        "utils": (
            held("iswrapper", "getTimeStrFromMillis", "getEnumTypeName", "floatMaxString",
                 "intMaxString", "longMaxString", "decimalMaxString"),
            "The helpers a program's own callbacks call.",
        ),
    }
    return {name: _module(name, contents, doc) for name, (contents, doc) in layout.items()}
