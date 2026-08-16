"""Both clients carry the same order and the same contract.

Two surfaces over one session drift the moment one is added to. A field on the
Rust client and not here is one a Python caller cannot set; a field here and
not there is one that does nothing. Either way a program written against one
and moved to the other behaves differently, which is the whole thing this
library exists to avoid.

Read from the sources rather than from a list kept beside them: a list is a
third thing to forget to update.
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[2]


def _fields(where: str, struct: str) -> set[str]:
    """The fields of a struct, wherever in a module it is written.

    Named by module rather than by file: a struct that moves to a sibling file
    during a refactor otherwise takes its parity check with it, and a check
    that raises is better than one that quietly compares nothing.
    """
    root = ROOT / where
    files = [root] if root.is_file() else sorted(root.rglob("*.rs"))
    for f in files:
        text = f.read_text()
        at = text.find(f"pub struct {struct} ")
        if at < 0:
            continue
        end = text.index("\n}", at)
        return set(re.findall(r"^\s*pub (\w+):", text[at:end], re.M))
    raise AssertionError(f"no `pub struct {struct}` anywhere under {where}")


def test_both_clients_carry_the_same_order():
    rust = _fields("src/types", "Order")
    python = _fields("src/python/compat", "Order")
    assert rust == python, (
        "an order field exists on one client and not the other: "
        f"rust only={sorted(rust - python)} python only={sorted(python - rust)}"
    )


#: The contract's own fields, which the reference client holds on a nested
#: contract and this binding holds flat beside the rest. A difference in shape,
#: not in what a caller can reach.
_ON_THE_CONTRACT = {
    "local_symbol", "primary_exchange", "right", "strike", "trading_class",
}

#: Beyond what the reference client has. An extra field breaks no program
#: written against that client, so these are kept — named here so that keeping
#: one stays a decision rather than an oversight.
_BEYOND_THE_REFERENCE = {
    # The first of the price-increment rules, beside the full list the
    # reference client states. Programs here already read it.
    "market_rule_id",
}


def test_both_clients_carry_the_same_contract_details():
    rust = _fields("src/types", "ContractDetails")
    python = (
        _fields("src/python/compat", "ContractDetails")
        - _ON_THE_CONTRACT
        - _BEYOND_THE_REFERENCE
    )
    assert rust == python, (
        "a contract-details field exists on one client and not the other: "
        f"rust only={sorted(rust - python)} python only={sorted(python - rust)}"
    )
