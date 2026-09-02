"""A program written against the reference client keeps its import lines.

That client does not publish one flat name. Its programs write
`from ibapi.client import EClient`, `from ibapi import wrapper` and then
`wrapper.EWrapper`, `from ibapi.contract import *`. All of it is on the first
page, so a layout that answers none of those lines means no such program ever
reached its first statement, whatever else was exported.

Under these names, porting is the one rename a person would guess at — `ibapi`
becomes `ibx` — and the vendor's own sample files import unchanged after it.
Nothing here is a second implementation: each module is a view of what `ibx`
already publishes, and `ibx.client.EClient is ibx.EClient`.
"""

import importlib

import ibx

# (module, the names a sample imports from it)
LAYOUT = [
    ("client", ["EClient"]),
    ("wrapper", ["EWrapper"]),
    ("contract", ["Contract", "ContractDetails", "ComboLeg", "DeltaNeutralContract"]),
    ("order", ["Order", "OrderComboLeg", "COMPETE_AGAINST_BEST_OFFSET_UP_TO_MID"]),
    ("order_state", ["OrderState"]),
    ("order_cancel", ["OrderCancel"]),
    ("order_condition", ["OrderCondition", "PriceCondition", "TimeCondition"]),
    ("execution", ["Execution", "ExecutionFilter"]),
    ("commission_and_fees_report", ["CommissionAndFeesReport"]),
    ("scanner", ["ScannerSubscription", "ScanData"]),
    ("tag_value", ["TagValue"]),
    ("ticktype", ["TickTypeEnum", "TickAttrib"]),
    ("account_summary_tags", ["AccountSummaryTags"]),
    ("object_implem", ["Object"]),
    ("common", ["BarData", "UNSET_DOUBLE", "TickerId", "MarketDataTypeEnum"]),
    ("const", ["UNSET_DOUBLE", "NO_VALID_ID", "MAX_MSG_LEN"]),
    ("utils", ["iswrapper", "getTimeStrFromMillis", "decimalMaxString"]),
]


def test_each_module_the_reference_client_publishes_is_importable():
    missing = []
    for name, held in LAYOUT:
        try:
            module = importlib.import_module(f"ibx.{name}")
        except ImportError as why:
            missing.append(f"ibx.{name}: {why}")
            continue
        for one in held:
            if not hasattr(module, one):
                missing.append(f"ibx.{name}.{one}")
    assert not missing, "\n".join(missing)


def test_a_module_is_a_view_and_not_a_second_implementation():
    # `from ibapi.client import EClient` and `from ibapi import EClient` name
    # one class there, and they have to name one here.
    from ibx.client import EClient
    from ibx.contract import Contract
    from ibx.execution import ExecutionFilter

    assert EClient is ibx.EClient
    assert Contract is ibx.Contract
    assert ExecutionFilter is ibx.ExecutionFilter


def test_the_module_is_reachable_as_an_attribute_too():
    # A program writes both spellings: `from ibapi import wrapper` then
    # `wrapper.EWrapper`, and `from ibapi.wrapper import EWrapper`.
    from ibx import order_condition, wrapper

    assert wrapper.EWrapper is ibx.EWrapper
    assert order_condition.OrderCondition.Price == 1


def test_a_star_import_from_one_of_them_brings_something():
    # The samples star-import four of these, so an empty module would pass the
    # import and leave the program with nothing.
    for name, _ in LAYOUT:
        module = importlib.import_module(f"ibx.{name}")
        assert getattr(module, "__all__", None), f"ibx.{name} publishes nothing"
