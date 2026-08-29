#!/usr/bin/env python3
"""Write down which wires this client speaks, from the source that speaks them.

A drop-in claim is a claim about messages, not about method names. This reads
the dispatch tables and the message builders and publishes what they cover, so
the claim can be checked rather than believed.

The page carries a completeness claim, so this errs toward failing loudly:
an unknown constant, an unbalanced block or a dispatch table it cannot find
raises rather than quietly publishing less than the truth.

Usage: python scripts/gen_wire_coverage.py
"""

import re
import sys

sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent))
from _paths import module, module_files  # noqa: E402
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "book" / "src" / "reference" / "wire-coverage.md"

# A message type means different things on different connections: `G` is an
# order replace on the trading connection and a binary tick payload on the
# market data one. Naming them from one table would mislabel half of them.
SHARED = {
    "0": "Heartbeat", "1": "Test request", "3": "Session reject", "5": "Logout",
    "A": "Logon", "U": "User message",
}
TRADING = SHARED | {
    "8": "Execution report", "9": "Cancel reject", "B": "News",
    "D": "New order", "F": "Order cancel", "G": "Order replace",
    "H": "Order mass status request", "V": "Market data request",
    "W": "Chart request", "Z": "Chart cancel",
    "c": "Security definition request", "d": "Security definition",
    "UT": "Account update", "UM": "Account update", "RL": "Account update",
    "UP": "Position update",
}
MARKET_DATA = SHARED | {
    "G": "Tick payload, binary", "L": "Ticker setup", "P": "Tick",
    "Q": "Subscription ack", "Y": "Subscription reject",
    "UT": "Account update", "UM": "Account update", "RL": "Account update",
    "UP": "Position update",
}
HISTORICAL = SHARED | {
    "E": "Historical payload", "G": "Bar payload", "W": "Chart response",
}

# Where the dispatch tables live: file, the text that starts the match, and the
# names to use for what it branches on.
DISPATCHERS = [
    ("trading connection", "src/engine/hot_loop/ccp", "match msg_type {", TRADING),
    ("market data connection", "src/engine/hot_loop/farm", "match msg_type", MARKET_DATA),
    ("historical connection", "src/engine/hot_loop/hmds", "match msg_type", HISTORICAL),
    # The fourth connection. Left out, the page said a type it handles is
    # neither sent nor handled anywhere — which is what the page tells a reader
    # an absence means.
    ("security definition connection", "src/engine/hot_loop/secdef", "match msg_type", SHARED),
]


def die(why: str) -> None:
    sys.exit(f"gen_wire_coverage: {why}")


def without_tests(text: str) -> str:
    """The source with every `#[cfg(test)]` block removed.

    A test fixture that builds a message is not this client sending one. Left
    in, they publish an inbound-only type as sent.
    """
    out, at = [], 0
    while True:
        found = text.find("#[cfg(test)]", at)
        if found < 0:
            out.append(text[at:])
            return "".join(out)
        out.append(text[at:found])
        brace = text.find("{", found)
        if brace < 0:
            return "".join(out)
        at = end_of_block(text, brace)


def end_of_block(text: str, open_brace: int) -> int:
    """The index just past the `}` matching the `{` at `open_brace`."""
    depth, i, n = 0, open_brace, len(text)
    while i < n:
        c = text[i]
        if c == '"':                      # skip string literals
            i += 1
            while i < n and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
        elif c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    die("a block never closes; the source did not parse as expected")


def sources() -> dict:
    return {
        f.relative_to(ROOT).as_posix(): without_tests(f.read_text(errors="ignore"))
        for f in (ROOT / "src").rglob("*.rs")
        if f.stem != "tests"
    }


def constants() -> dict:
    src = (ROOT / "src" / "protocol" / "fix.rs").read_text()
    return dict(re.findall(r'pub const (MSG_[A-Z_]+): &str = "([^"]+)";', src))


def sent(consts: dict, srcs: dict) -> set:
    """Message types this client builds, from production code only.

    Two shapes reach the wire: the named tag and the literal 35.
    """
    found = set()
    for text in srcs.values():
        for m in re.finditer(r'\(\s*(?:fix::)?TAG_MSG_TYPE\s*,\s*(?:fix::)?(MSG_[A-Z_]+|"([A-Za-z0-9]+)")', text):
            if m.group(2):
                found.add(m.group(2))
            elif m.group(1) in consts:
                found.add(consts[m.group(1)])
            else:
                die(f"{m.group(1)} is not declared in src/protocol/fix.rs, so its wire value is unknown")
        for m in re.finditer(r'\(\s*35\s*,\s*"([A-Za-z0-9]+)"\s*\)', text):
            found.add(m.group(1))
    return found


def sent_subtypes(srcs: dict) -> list:
    """Every subtype this client writes on tag 6040.

    The tag is written as its number and as the name the module that owns it
    gives it, and reading only the number missed every request sent under the
    name — a page saying those subtypes are neither sent nor handled.
    """
    # The tag under its number and under either name a module gives it, however
    # the path to that name is spelled.
    named_tag = r'(?:6040|(?:\w+::)*TAG_SUB_PROTOCOL|(?:\w+::)*TAG_IB_COMM_TYPE)'
    found = set()
    for text in srcs.values():
        found |= set(re.findall(rf'\(\s*{named_tag}\s*,\s*&?"(\d+)"\s*\)', text))
        # A constant named as a sub-protocol is one: it exists to be written on
        # that tag, and is passed as a value rather than spelled out beside it,
        # so a reading that looked only for the literal said the subtype was
        # neither sent nor handled anywhere. Written as a number or as text,
        # because both spellings are in use.
        #
        # `TAG_` names the tag itself rather than a subtype travelling under it,
        # and reading it as one published the tag's own number as a subtype.
        found |= set(re.findall(
            r'const (?!TAG_)\w*SUB_PROTOCOL\w*: (?:u32 = (\d+)|&str = "(\d+)")\s*;', text,
        ) and [
            g for pair in re.findall(
                r'const (?!TAG_)\w*SUB_PROTOCOL\w*: (?:u32 = (\d+)|&str = "(\d+)")\s*;', text,
            ) for g in pair if g
        ])
    return sorted(found, key=int)


def dispatch_block(path: str, start: str) -> str:
    text = without_tests(
        "\n".join(f.read_text(errors="ignore") for f in module_files(path))
    )
    at = text.find(start)
    if at < 0:
        die(f"no dispatch table matching {start!r} in {path}")
    brace = text.find("{", at + len(start) - 1)
    return text[at:end_of_block(text, brace)]


ARM = re.compile(
    r'^(\s+)((?:(?:fix::MSG_[A-Z_]+|b?"[A-Za-z0-9]+")\s*\|\s*)*'
    r'(?:fix::MSG_[A-Z_]+|b?"[A-Za-z0-9]+"))\s*=>'
)


def matched_arms(block: str) -> list:
    """Every match arm in a dispatch block, with how deeply it is nested.

    A user message's subtypes are matched inside the arm that selects user
    messages, so depth is what tells a message type from a subtype. Reading
    them all as types publishes subtypes as message types.
    """
    out = []
    for line in block.splitlines():
        m = ARM.match(line)
        if m:
            out.append((len(m.group(1)), m.group(2)))
    return out


def name_of(part: str, consts: dict) -> str:
    part = part.strip()
    if part.startswith("fix::"):
        name = part[5:]
        if name not in consts:
            die(f"{name} is not declared in src/protocol/fix.rs")
        return consts[name]
    return part.removeprefix("b").strip('"')


def arms(block: str, consts: dict) -> set:
    """The message types a dispatch table branches on, outermost arms only."""
    all_arms = matched_arms(block)
    if not all_arms:
        die("a dispatch block has no match arms; the source did not parse as expected")
    top = min(depth for depth, _ in all_arms)
    return {
        name_of(part, consts)
        for depth, arm in all_arms if depth == top
        for part in arm.split("|")
    }


def handled_subtypes(block: str, consts: dict) -> list:
    """The user-message subtypes it branches on: numeric, and nested."""
    all_arms = matched_arms(block)
    if not all_arms:
        return []
    top = min(depth for depth, _ in all_arms)
    found = {
        name_of(part, consts)
        for depth, arm in all_arms if depth > top
        for part in arm.split("|")
    }
    return sorted((v for v in found if v.isdigit()), key=int)


def table(title: str, values, names: dict) -> list:
    lines = [f"### {title}", "", "| Type | Meaning |", "| --- | --- |"]
    for v in sorted(values, key=lambda x: (len(x), x)):
        lines.append(f"| `{v}` | {names.get(v, 'not named here')} |")
    return lines + [""]


def listing(title: str, values, note: str = "") -> list:
    lines = [f"### {title}", ""]
    if note:
        lines += [note, ""]
    return lines + ["| Subtype |", "| --- |"] + [f"| `{v}` |" for v in values] + [""]


def main() -> None:
    consts, srcs = constants(), sources()

    lines = [
        "# Wire coverage",
        "",
        "*Auto-generated from source — do not edit.*",
        "",
        "Which messages this client speaks. A claim to replace the vendor's",
        "gateway is a claim about messages rather than about method names, so",
        "this is taken from the dispatch tables and the message builders",
        "themselves, and CI regenerates it and fails if it has drifted.",
        "",
        "Test code is excluded: a fixture that builds a message is not this",
        "client sending one.",
        "",
        "What this does **not** establish: a type absent here is one this client",
        "neither sends nor handles, which is not the same as one the venue never",
        "sends. Settling that needs a record of everything the venue can send.",
        "",
        "How it is read: the four connections' dispatch tables, the subtypes",
        "written beside the tag that carries them under either of its names, and",
        "the constants named as sub-protocols. A subtype reaching the wire by",
        "some other shape would be missing from this page, and one has been:",
        "the calendar's, passed as a named constant rather than spelled out,",
        "which is why constants are read too.",
        "",
        "## Sent",
        "",
    ]
    lines += table("Message types", sent(consts, srcs), TRADING)
    lines += listing(
        "User-message subtypes",
        sent_subtypes(srcs),
        "A user message carries what it is for on tag 6040.",
    )

    lines += ["## Handled", ""]
    # The logon exchange runs before the dispatch tables exist and tests the
    # message type directly, so it is stated rather than extracted.
    logon = sorted(set(re.findall(
        r'msg_type == "([A-Za-z0-9]+)"|== Some\("([A-Za-z0-9]+)"\)',
        without_tests("\n".join(
            f.read_text(errors="ignore") for f in module_files("src/gateway")
        )),
    )))
    logon = sorted({a or b for a, b in logon})
    lines += table(
        "During the logon exchange, before the dispatch tables run", logon, TRADING,
    )
    for label, path, start, names in DISPATCHERS:
        block = dispatch_block(path, start)
        lines += table(f"On the {label}", arms(block, consts), names)
        subs = handled_subtypes(block, consts)
        if subs:
            lines += listing(f"User-message subtypes on the {label}", subs)

    OUT.write_text("\n".join(lines) + "\n")
    print(f"{OUT.relative_to(ROOT)} — {len(sent(consts, srcs))} types sent")


if __name__ == "__main__":
    main()
