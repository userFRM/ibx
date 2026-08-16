#!/usr/bin/env python3
"""Count which arguments of a call reach anything.

`gen_wire_reach.py` counts calls and `gen_order_field_reach.py` counts the
fields of an order. Neither sees the third way a caller can be ignored: a call
that reaches the venue, carrying an argument the body never reads.

Three were found by hand before this existed. `format_date` chose between a
date and a number and was dropped on both surfaces, so a caller asking for
seconds since the epoch was handed a date. `number_of_ticks` and `ignore_size`
asked for a stream this protocol cannot ask for. `api_only` asked for a filter
nothing here can apply.

Every argument of every call falls into one of three kinds:

  read      the body names it somewhere other than a discard
  stated    the body discards it and the doc comment says so, by name
  dropped   the body discards it and nothing says so

`dropped` is the one that matters, and it is held at zero. An argument a
caller can set and nothing reads is the argument form of a silent call: the
request goes out, the answer comes back, and what was asked for is gone.

Writes target/gates/arg-reach.md, which is a report and not the gate: this
exits non-zero on its own findings.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "target/gates/arg-reach.md"

#: Where a caller's call is taken. Both surfaces, because an argument dropped
#: on one and read on the other is a difference between the two clients.
SURFACES = {
    "Rust": ROOT / "src/api/client",
    "Python": ROOT / "src/python/compat/client",
}

#: Arguments every call carries that are not the caller's data.
NOT_DATA = {"self", "py", "wrapper", "req_id", "order_id", "contract", "order"}

CALL = re.compile(
    r"((?:^[ \t]*///[^\n]*\n)*)"              # its doc comment, if any
    r"(?:^[ \t]*#\[[^\n]*\n)*"                 # any attributes under it
    r"[ \t]*(?:pub(?:\(crate\))? )?fn ([a-z_0-9]+)\(([^)]*)\)",
    re.M,
)


def _body(text: str, at: int) -> str:
    """The call's body: from its brace to the next item at the same depth."""
    start = text.find("{", at)
    if start < 0:
        return ""
    depth, i = 0, start
    while i < len(text):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
        i += 1
    return text[start:]


def _params(raw: str) -> list[str]:
    """Parameter names, less the ones that are not a caller's data.

    Only `name: type` counts. A bare type is a closure's or a tuple's, not
    something a caller names — read as a parameter it invents an argument
    nobody can pass, and a gate that reports those is one people learn to
    ignore.
    """
    names, depth, part = [], 0, ""
    for ch in raw:
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        if ch == "," and depth == 0:
            names.append(part)
            part = ""
        else:
            part += ch
    names.append(part)

    out = []
    for piece in names:
        piece = piece.strip()
        if not piece or ":" not in piece:
            continue
        name = piece.split(":", 1)[0].strip()
        if not re.fullmatch(r"_?[a-z][a-z_0-9]*", name):
            continue
        if name.lstrip("_") in NOT_DATA:
            continue
        out.append(name)
    return out


def _kind(name: str, body: str, doc: str) -> str:
    # `_override` and `override_` are one argument written two ways: the
    # leading underscore says the body ignores it, the trailing one keeps it
    # off a keyword. Neither is part of the name.
    bare = name.strip("_")
    # The name as written, either way round.
    written = rf"\b_?{re.escape(bare)}_?\b"
    discarded = (
        name.startswith("_")
        or re.search(rf"let _ = \(?[^;]*{written}", body)
    )
    if not discarded:
        # Named anywhere in the body that is not the discard itself.
        if re.search(written, body):
            return "read"
        return "dropped"
    named = re.search(written, doc)
    return "stated" if named else "dropped"


#: Where the client's own calls live. A callback carries arguments the venue
#: set, not the caller, so a wrapper method that ignores one is not a caller
#: being ignored.
IMPL = re.compile(r"^\s*impl(?:<[^>]*>)? (?:\w+ for )?(\w+)", re.M)


def _impl_at(text: str, at: int) -> str:
    """Which `impl` block a position falls in."""
    last = ""
    for m in IMPL.finditer(text):
        if m.start() > at:
            break
        last = m.group(1)
    return last


def collect() -> list[tuple[str, str, str, str]]:
    rows = []
    for surface, directory in SURFACES.items():
        for path in sorted(directory.glob("*.rs")):
            if path.stem == "tests":
                continue
            text = path.read_text()
            for m in CALL.finditer(text):
                doc, call, raw = m.group(1), m.group(2), m.group(3)
                if call.startswith("_test") or call.startswith("new"):
                    continue
                if _impl_at(text, m.start()) != "EClient":
                    continue
                body = _body(text, m.end())
                for name in _params(raw):
                    rows.append((surface, call, name, _kind(name, body, doc)))
    return rows


def main() -> int:
    rows = collect()
    dropped = [r for r in rows if r[3] == "dropped"]
    counts = {k: sum(1 for r in rows if r[3] == k) for k in ("read", "stated", "dropped")}

    lines = [
        "# What an argument reaches",
        "",
        "Generated by `scripts/gen_arg_reach.py`. Do not edit.",
        "",
        "A call can reach the venue while an argument a caller set never reaches",
        "anything. The request goes out, the answer comes back, and what was asked",
        "for is gone — with nothing to say so.",
        "",
        "| Kind | Count | Meaning |",
        "| --- | ---: | --- |",
        f"| read | {counts['read']} | the call reads it |",
        f"| stated | {counts['stated']} | the call does not read it, and says so by name |",
        f"| dropped | {counts['dropped']} | the call does not read it, and nothing says so |",
        "",
        "`dropped` is held at zero. An argument that cannot be served is said so",
        "in the doc comment, which is where the generated API reference carries it",
        "to a reader.",
        "",
    ]
    stated = sorted({(r[1], r[2]) for r in rows if r[3] == "stated"})
    if stated:
        lines += ["## Taken and not applied", ""]
        lines += [f"- `{call}` — `{name}`" for call, name in stated]
        lines += [""]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines))

    print(f"{len(rows)} call arguments: read={counts['read']} "
          f"stated={counts['stated']} dropped={counts['dropped']}")
    if dropped:
        print("\nAn argument a caller can set reaches nothing, and nothing says so:")
        for surface, call, name, _ in dropped:
            print(f"  {surface} {call}({name})")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
