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


#: An argument a surface takes and will not send is refused with this sentence,
#: which names the argument first. Read from the sources for the same reason the
#: fields are: a list beside them is a third thing to forget.
_REFUSED = re.compile(r'"([a-z_]+)=?\{?[a-z_]*\}? is not carried by this protocol')


def _refused(where: str) -> set[str]:
    return {
        m.group(1)
        for f in (ROOT / where).rglob("*.rs")
        for m in _REFUSED.finditer(f.read_text())
    }


def test_both_clients_refuse_the_same_arguments():
    """An argument one client refuses and the other accepts is the worse half
    of the same bug the fields test covers. Refused on one surface, the caller
    is told; accepted on the other, they are answered with a stream or an order
    that is not the one they asked for, and nothing says so."""
    rust = _refused("src/api/client")
    python = _refused("src/python/compat/client")
    assert rust == python, (
        "an argument is refused by one client and accepted by the other: "
        f"rust only={sorted(rust - python)} python only={sorted(python - rust)}"
    )


#: Filled at the call site instead of in the conversion, because the Python
#: value is an object and reading one needs the interpreter the conversion does
#: not hold. Named here so that adding to the list stays a decision.
_FILLED_BY_THE_CALLER = {"conditions", "order_combo_legs", "order_misc_options"}


def _to_api_body() -> str:
    text = (ROOT / "src/python/compat/class_orders.rs").read_text()
    at = text.index("pub fn to_api(&self)")
    return text[at:text.index("\n    }\n", at)]


def test_every_order_field_reaches_the_rust_order():
    """A field a Python caller sets has to arrive on the order that is sent.

    The conversion used to end in `..Default::default()`, and a hundred and ten
    of the hundred and fifty-four fields fell into it: set from Python, reset
    before anything read them, and gone with no error. The struct-field test
    above passed throughout, because both sides declared the field — only the
    conversion between them dropped it.
    """
    body = _to_api_body()
    assert ".." not in body.split("crate::types::model::Order {", 1)[1], (
        "the conversion ends in a struct-update fallback, so a field nobody "
        "copied is silently defaulted instead of failing to compile"
    )
    missing = sorted(
        f for f in _fields("src/python/compat", "Order")
        if f in _fields("src/types", "Order")
        and f not in _FILLED_BY_THE_CALLER
        and not re.search(rf"^\s*{f}:", body, re.M)
    )
    assert not missing, f"set from Python and never reaching the order sent: {missing}"


def test_every_order_field_comes_back_from_the_engine():
    """An order handed back to Python is the order the engine holds.

    A callback used to rebuild one field by field and default the rest, so a
    caller reading their own open orders got an order missing most of what they
    placed — and placing it again placed something else. The reverse of the
    conversion above, and it went wrong the same way.
    """
    source = (ROOT / "src/python/compat/class_orders.rs").read_text()
    at = source.index("pub(crate) fn from_api(a: &crate::types::model::Order)")
    body = source[at:source.index("\n    }\n", at)]
    assert ".." not in body.split("Self {", 1)[1], (
        "the conversion back ends in a struct-update fallback, so a field "
        "nobody copied is silently defaulted instead of failing to compile"
    )
    missing = sorted(
        f for f in _fields("src/python/compat", "Order")
        if f not in _FILLED_BY_THE_CALLER
        and not re.search(rf"^\s*{f}:", body, re.M)
    )
    assert not missing, f"held by the engine and not handed back: {missing}"


def test_what_cannot_be_read_is_refused_rather_than_emptied():
    """The three the caller fills carry Python objects, and an object this
    client cannot read is a refusal.

    Read as absent, a combination leg goes out unpriced and a tag the protocol
    does not carry stops being refused for stating it — the silent transform
    these gates exist to prevent, reintroduced by the code that fills them.
    """
    source = (ROOT / "src/python/compat/class_orders.rs").read_text()
    for converter in ("convert_order_combo_legs", "convert_misc_options",
                      "convert_conditions"):
        at = source.index(f"pub fn {converter}(")
        body = source[at:source.index("\n    }\n", at)]
        assert "Result<" in body.split("{", 1)[0], f"{converter} cannot report a failure"
        for silent in ("unwrap_or", "filter_map", "unwrap_or_default", ".ok()?"):
            assert silent not in body, f"{converter} turns an unreadable value into {silent}"


def test_what_the_caller_fills_is_filled_by_the_caller():
    """The three the conversion leaves empty are filled where it says they are.

    Left to the conversion they would be silently empty, which is what the test
    above forbids; named as exceptions and then filled nowhere, they would be
    silently empty with the exception saying so.
    """
    at_the_call_site = (ROOT / "src/python/compat/client/orders.rs").read_text()
    for field in _FILLED_BY_THE_CALLER:
        assert re.search(rf"api_order\.{field}\s*=", at_the_call_site), (
            f"{field} is excused from the conversion and assigned nowhere"
        )
