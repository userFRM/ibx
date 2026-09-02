"""The protocol level names what this client carries, and what it cannot apply
is refused rather than dropped.

The reference client's `serverVersion()` is the API level of the process a
program talks to. Here that process is this client, so the number is the newest
of the reference's gates whose feature is carried, and a caller comparing
against a gate is told the truth about everything below it or refused by name.
"""

import ibx


class _Recorder(ibx.EWrapper):
    def __init__(self):
        super().__init__()
        self.errors = []
        self.ended = []

    def error(self, reqId, errorTime, code, msg, advanced=""):
        self.errors.append((reqId, code, msg))

    def execDetailsEnd(self, reqId):
        self.ended.append(reqId)


def test_the_level_is_the_newest_gate_carried_and_none_before_a_session():
    c = ibx.EClient(ibx.EWrapper())
    assert c.serverVersion() is None
    c._test_connect()
    # MIN_SERVER_VER_ADDITIONAL_ORDER_PARAMS_2 in the reference's table. The
    # exception list below it is the doc on the call.
    assert c.serverVersion() == 217
    c.disconnect()
    assert c.serverVersion() is None


def test_a_window_in_days_or_dates_is_refused_not_dropped():
    w = _Recorder()
    c = ibx.EClient(w)
    c._test_connect()

    stated = ibx.ExecutionFilter()
    stated.lastNDays = 3
    c.reqExecutions(1, stated)
    assert [e for e in w.errors if e[0] == 1 and e[1] == 321 and "lastNDays" in e[2]], w.errors

    dated = ibx.ExecutionFilter()
    dated.specificDates = ["20260901"]
    c.reqExecutions(2, dated)
    assert [e for e in w.errors if e[0] == 2 and e[1] == 321], w.errors


def test_the_reference_defaults_pass_through():
    w = _Recorder()
    c = ibx.EClient(w)
    c._test_connect()
    # UNSET_INTEGER and None, which is what a filter that states no window carries.
    c.reqExecutions(3, ibx.ExecutionFilter())
    c._test_dispatch_once()
    assert not w.errors, w.errors
    assert 3 in w.ended


def test_every_surface_answers_the_question_with_one_number():
    # Three surfaces answer "what protocol level am I talking to". One of them
    # used to answer with the build this client states at logon — a number on
    # another scale, above every level there is — so a program gating a feature
    # on it was told every feature exists.
    import subprocess
    import pathlib

    root = pathlib.Path(__file__).resolve().parents[2]
    stated = subprocess.run(
        ["grep", "-rn", "PROTOCOL_LEVEL", str(root / "src")],
        capture_output=True, text=True,
    ).stdout
    answering = [line for line in stated.splitlines() if "client_core::PROTOCOL_LEVEL" in line]
    assert len(answering) >= 2, f"a surface answers with something else:\n{stated}"
    assert "settings().build" not in (root / "src/api/direct.rs").read_text(), \
        "a surface still answers the protocol question with the build number"
