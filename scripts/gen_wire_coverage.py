#!/usr/bin/env python3
"""Write down which wires this client speaks, from the source that speaks them.

A claim to replace the gateway is a claim about messages, not about method
names. This reads the dispatch tables and the message builders and publishes
what they actually cover, so the claim can be checked rather than believed.

The page carries a completeness claim, so this errs toward failing loudly:
an unknown constant, an unbalanced block or a dispatch table it cannot find
raises rather than quietly publishing less than the truth.

Usage: python scripts/gen_wire_coverage.py
"""

import re
import sys
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
    ("trading connection", "src/engine/hot_loop/ccp.rs", "match msg_type {", TRADING),
    ("market data connection", "src/engine/hot_loop/farm.rs", "match msg_type", MARKET_DATA),
    ("historical connection", "src/engine/hot_loop/hmds.rs", "match msg_type", HISTORICAL),
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
    found = set()
    for text in srcs.values():
        found |= set(re.findall(r'\(\s*6040\s*,\s*&?"(\d+)"\s*\)', text))
    return sorted(found, key=int)


def dispatch_block(path: str, start: str) -> str:
    text = without_tests((ROOT / path).read_text(errors="ignore"))
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
        "sends. That comparison needs the vendor's own inventory.",
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
        without_tests((ROOT / "src" / "gateway.rs").read_text(errors="ignore")),
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
