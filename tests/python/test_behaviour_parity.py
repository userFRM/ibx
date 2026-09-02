"""Both clients answer a request that cannot be served.

The surface test compares what a caller can set. This compares what happens
when a call cannot be served. A call that returns without telling the caller
leaves them waiting on a callback that never arrives, which is
indistinguishable from a slow venue.

The two mechanisms: the request surface returns `Result<(), Refusal>`, the
binding reports on the error callback. A method that only forwards to a sibling
answers however that sibling does.
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[2]

#: The one binding call that answers by raising rather than reporting. `connect`
#: has no request id to report against, so a failure raises.
#: Calls that answer a failure by raising rather than by reporting. A blocking
#: call has a caller waiting on its return, so raising reaches them; the
#: request surface answers on the error callback because nobody is waiting.
#: `corporate_actions` sends its own command rather than going through the
#: reporting request call, because taking the session and the sender apart let
#: a reconnect put a request and its answer on two different sessions.
_ANSWERS_BY_RAISING = {"connect", "corporate_actions"}

#: Calls the request surface leaves quiet where the binding reports. Each takes
#: a wrapper it could report on and returns after a log line. Listed as an open
#: gap rather than an intended difference, so a new divergence still fails.
_ONE_SIDED_ON_THE_REQUEST_SURFACE = {
    "req_positions",
    "req_positions_multi",
    "req_account_updates_multi",
    "req_ids",
    "req_managed_accts",
    "req_current_time",
    "req_current_time_in_millis",
    "req_executions",
    "req_auto_open_orders",
    # Answered under 504 by the binding. The request surface's twins sit in
    # reference.rs and take the same guard as the others once that file is
    # free; listed here until then.
    "req_news_bulletins",
    "cancel_news_bulletins",
}


def _methods(pattern: str, *globs: str) -> dict[str, str]:
    found: dict[str, str] = {}
    for glob in globs:
        for path in sorted(ROOT.glob(glob)):
            text = path.read_text()
            for m in re.finditer(pattern, text):
                start = m.end()
                after = [text.find(s, start) for s in ("\n    fn ", "\n    pub fn ", "\n    pub(crate) fn ")]
                after = [x for x in after if x > 0] + [len(text)]
                found[m.group(1)] = text[start : min(after)]
    return found


#: How the binding reports a request it will not serve: on the error callback,
#: returning normally, as the reference client does. `tx_or_report` and
#: `report_refusal` are the current spellings; the rest predate them.
_REPORTS = (
    "report_refusal", "tx_or_report", "report_unserviceable",
    "wrapper.error", '"error"', "report_reason", "push_historical_error",
)


#: How each surface's methods are found.
#:
#: Private helpers are included on both sides. Collecting only `pub fn` on the
#: request surface left a public call that does its work in a private sibling
#: tracing to nothing, so it read as silent while its opposite number, whose
#: helper was visible, read as answering — a difference between the two
#: patterns rather than between the two clients. Named once so the comparison
#: and the check guarding it cannot come to read different sets.
_RUST_METHODS = r"\n    (?:pub(?:\(crate\))? )?fn ([a-z_0-9]+)\("
_PY_METHODS = r"\n    (?:pub |pub\(crate\) )?fn ([a-z_0-9]+)\("

#: The request surface itself, which is what the two clients are compared on.
#: The index above is wider so a call can be traced into the helpers it hands
#: to; comparing over that index instead would pair private helpers, and a
#: helper that exists on one side only is not a caller-visible difference.
_RUST_SURFACE = r"\n    pub fn ([a-z_0-9]+)\("


def _answers(name: str, methods: dict[str, str], through: tuple[str, ...] = ()) -> bool:
    """Whether a call that cannot be served lets the caller know.

    Recognises both mechanisms: a `Result` return on the request surface, a
    report on the error callback in the binding. A predicate that recognises
    only one reads most pairs as silent on both sides and compares nothing.
    """
    body = methods[name]
    if "-> Result<" in body or any(k in body for k in _REPORTS):
        return True
    # A call whose work is one call to a sibling answers however that one does.
    # Without this a thin forwarder reads as silent while the method it hands to
    # reports, and the pair reads as differing when it does not.
    return any(
        to in methods and to not in through and to != name
        and _answers(to, methods, through + (name,))
        for to in re.findall(r"\bself\.([a-z_0-9]+)\(", body)
    )


def test_a_call_that_cannot_be_served_answers_on_both_clients():
    rust = _methods(_RUST_METHODS, "src/api/client/*.rs")
    python = _methods(_PY_METHODS, "src/python/compat/client/*.rs")
    surface = _methods(_RUST_SURFACE, "src/api/client/*.rs")

    ignored = _ANSWERS_BY_RAISING | _ONE_SIDED_ON_THE_REQUEST_SURFACE
    differs = sorted(
        name
        for name in set(surface) & set(python)
        if name not in ignored
        and _answers(name, rust) != _answers(name, python)
    )
    assert not differs, (
        "one client answers a failure and the other does not, so a caller of "
        f"the quieter one waits on a callback that never comes: {differs}"
    )


def test_the_gate_can_still_tell_the_two_apart():
    """A predicate that recognises nothing passes everything.

    Guards the comparison above: reading the mechanisms the code uses must find
    most calls answering on each surface. Finding few means this file has
    stopped reading that surface, and every pair then matches as silent.
    """
    rust = _methods(_RUST_METHODS, "src/api/client/*.rs")
    python = _methods(_PY_METHODS, "src/python/compat/client/*.rs")
    common = set(_methods(_RUST_SURFACE, "src/api/client/*.rs")) & set(python)
    assert len(common) > 50, f"the two surfaces stopped overlapping: {len(common)}"
    for surface, methods in (("request", rust), ("binding", python)):
        answering = sum(_answers(n, methods) for n in common)
        assert answering > len(common) // 2, (
            f"the {surface} surface reads as answering {answering} of "
            f"{len(common)} calls, so this file is not reading it"
        )
