"""Both clients answer a request the same way, not just carry the same fields.

The surface test compares what a caller can set. This compares what happens
when a call cannot be served: one client answering on the error channel while
the other writes a line to a log and returns leaves a caller waiting on a
callback that will never come — and looks identical to a slow venue.

It has found three: a market rule nobody has seen, and the two advisor
requests, which one client sent to the venue while the other refused them as
unwired.
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parents[2]

#: Calls whose two sides differ on purpose, with the reason. Named here so a
#: difference stays a decision rather than an oversight.
_DIFFER_ON_PURPOSE = {
    # The binding takes a request id from the caller and answers refusals
    # against it; the Rust client returns the reason to the caller directly.
    "req_wsh_event_data",
    "update_display_group",
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


def _answers_a_failure(body: str) -> bool:
    return (
        "wrapper.error" in body
        or '"error"' in body
        or "report_reason" in body
        or "push_historical_error" in body
    )


def test_a_call_that_cannot_be_served_answers_on_both_clients():
    rust = _methods(r"\n    pub fn ([a-z_0-9]+)\(", "src/api/client/*.rs")
    python = _methods(
        r"\n    (?:pub |pub\(crate\) )?fn ([a-z_0-9]+)\(", "src/python/compat/client/*.rs"
    )

    differs = sorted(
        name
        for name in set(rust) & set(python)
        if name not in _DIFFER_ON_PURPOSE
        and _answers_a_failure(rust[name]) != _answers_a_failure(python[name])
    )
    assert not differs, (
        "one client answers a failure and the other does not, so a caller of "
        f"the quieter one waits on a callback that never comes: {differs}"
    )
