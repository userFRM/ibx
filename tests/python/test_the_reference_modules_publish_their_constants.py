"""The reference client's modules publish their constants and enumerations here too.

A program star-imports `ibx.order` and writes `order.origin = CUSTOMER`, as
that client's own `Order.__init__` does; it compares a scan's row count with
`NO_ROW_NUMBER_SPECIFIED`, a fill's exercise type with
`OptionExerciseType.NoneItem`, a fund's class with `FundAssetType.Equity`.
None of those names existed here, so each such line was a NameError or, for
the enumerations, a comparison that could never hold.

Run: pytest tests/python/test_the_reference_modules_publish_their_constants.py -v
"""

from ibx import contract, execution, news, order, scanner
from ibx import ContractDetails, Execution, Order, ScannerSubscription


def test_the_order_module_publishes_the_origin_and_auction_constants():
    assert (order.CUSTOMER, order.FIRM, order.UNKNOWN) == (0, 1, 2)
    assert (order.AUCTION_UNSET, order.AUCTION_MATCH, order.AUCTION_IMPROVEMENT, order.AUCTION_TRANSPARENT) == (0, 1, 2, 3)
    made = Order()
    assert made.origin == order.CUSTOMER
    assert made.auctionStrategy == order.AUCTION_UNSET


def test_the_contract_module_publishes_the_leg_positions():
    assert (contract.SAME_POS, contract.OPEN_POS, contract.CLOSE_POS, contract.UNKNOWN_POS) == (0, 1, 2, 3)


def test_the_scanner_and_news_modules_publish_their_constants():
    assert scanner.NO_ROW_NUMBER_SPECIFIED == -1
    assert ScannerSubscription().numberOfRows == scanner.NO_ROW_NUMBER_SPECIFIED
    assert (news.NEWS_MSG, news.EXCHANGE_AVAIL_MSG, news.EXCHANGE_UNAVAIL_MSG) == (1, 2, 3)


def test_an_exercise_type_is_the_enumeration_member():
    kinds = execution.OptionExerciseType
    fill = Execution()
    assert fill.optExerciseOrLapseType is kinds.NoneItem
    fill.optExerciseOrLapseType = kinds.Lapse
    assert fill.optExerciseOrLapseType is kinds.Lapse
    fill.opt_exercise_or_lapse_type = 100
    assert fill.optExerciseOrLapseType is kinds.Assigned
    fill.opt_exercise_or_lapse_type = 12345
    assert fill.optExerciseOrLapseType is kinds.NoneItem, "an unlisted code reads as the first member, as it does there"


def test_a_funds_class_and_policy_are_the_enumeration_members():
    details = ContractDetails()
    assert details.fundAssetType is contract.FundAssetType.NoneItem
    assert details.fundDistributionPolicyIndicator is contract.FundDistributionPolicyIndicator.NoneItem
    details.fundAssetType = contract.FundAssetType.Equity
    assert details.fundAssetType is contract.FundAssetType.Equity
    details.fund_asset_type = "001"
    assert details.fundAssetType is contract.FundAssetType.MoneyMarket
    details.fundDistributionPolicyIndicator = "Y"
    assert details.fundDistributionPolicyIndicator is contract.FundDistributionPolicyIndicator.IncomeFund
