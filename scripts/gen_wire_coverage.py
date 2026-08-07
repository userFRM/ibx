#!/usr/bin/env python3
"""Write down which wires this client speaks, from the source that speaks them.

A claim to replace the gateway is a claim about messages, not about method
names. This reads the dispatch tables and the message builders and publishes
what they actually cover, so the claim can be checked rather than believed.

Usage: python scripts/gen_wire_coverage.py
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "book" / "src" / "reference" / "wire-coverage.md"

# What a message type means depends on the connection it arrives on: `G` is an
# order replace on the trading connection and a binary tick payload on the
# market data one. Naming them from one table would mislabel half of them.
SHARED = {
    "0": "Heartbeat", "1": "Test request", "3": "Session reject", "5": "Logout",
    "A": "Logon", "U": "User message",
}
TRADING = SHARED | {
    "8": "Execution report", "9": "Cancel reject", "B": "News",
    "D": "New order", "F": "Order cancel", "G": "Order replace",
    "V": "Market data request", "W": "Chart request", "Z": "Chart cancel",
    "c": "Security definition request", "d": "Security definition",
    "E": "Order list", "I": "Mass status",
    "UT": "Account update", "UM": "Account update", "RL": "Account update",
    "UP": "Position update",
}
MARKET_DATA = SHARED | {
    "G": "Tick payload, binary", "L": "Ticker setup", "P": "Tick",
    "Q": "Subscription ack", "Y": "Subscription reject",
    "RL": "Account update", "UP": "Position update",
}


def constants() -> dict:
    """MSG_* constant name to its wire value."""
    src = (ROOT / "src" / "protocol" / "fix.rs").read_text()
    return dict(re.findall(r'pub const (MSG_[A-Z_]+): &str = "([^"]+)";', src))


def outbound(consts: dict) -> dict:
    """Message types this client builds, and where."""
    found = {}
    for f in (ROOT / "src").rglob("*.rs"):
        if f.stem == "tests":
            continue
        text = f.read_text(errors="ignore")
        for m in re.finditer(r'TAG_MSG_TYPE, (?:fix::)?(MSG_[A-Z_]+|"([A-Za-z0-9]+)")', text):
            value = consts.get(m.group(1), m.group(2))
            if value:
                found.setdefault(value, set()).add(f.relative_to(ROOT).as_posix())
    return found


def subtypes(pattern: str, files: list[Path]) -> set:
    out = set()
    for f in files:
        for m in re.finditer(pattern, f.read_text(errors="ignore")):
            out.add(m.group(1))
    return out


def inbound_arms(path: Path, start: str) -> list:
    """The message types a dispatch table branches on."""
    text = path.read_text(errors="ignore")
    at = text.find(start)
    if at < 0:
        return []
    window = text[at:at + 12000]
    consts = constants()
    out = []
    for line in window.splitlines():
        m = re.match(r'\s{8,12}((?:(?:fix::MSG_[A-Z_]+|b?"[A-Za-z0-9]+")\s*\|\s*)*(?:fix::MSG_[A-Z_]+|b?"[A-Za-z0-9]+"))\s*=>', line)
        if not m:
            continue
        for part in m.group(1).split("|"):
            part = part.strip()
            if part.startswith("fix::"):
                out.append(consts.get(part[5:], part))
            else:
                out.append(part.strip('b"'))
    return sorted(set(out))


def table(title: str, values, names: dict, note: str = "") -> list:
    lines = [f"### {title}", ""]
    if note:
        lines += [note, ""]
    lines += ["| Type | Meaning |", "| --- | --- |"]
    for v in sorted(values, key=lambda x: (len(x), x)):
        lines.append(f"| `{v}` | {names.get(v, 'not named here')} |")
    lines.append("")
    return lines


def main() -> None:
    consts = constants()
    out_msgs = outbound(consts)
    ccp = ROOT / "src" / "engine" / "hot_loop" / "ccp.rs"
    farm = ROOT / "src" / "engine" / "hot_loop" / "farm.rs"

    out_subtypes = sorted(
        subtypes(r'\(6040, "(\d+)"\)', list((ROOT / "src").rglob("*.rs"))),
        key=int,
    )
    in_subtypes = sorted(
        subtypes(r'\n\s{20,}"(\d+)" =>', [ccp]),
        key=int,
    )

    lines = [
        "# Wire coverage",
        "",
        "*Auto-generated from source — do not edit.*",
        "",
        "Which messages this client speaks. A claim to replace the vendor's",
        "gateway is a claim about messages rather than about method names, so",
        "this is taken from the dispatch tables and the message builders",
        "themselves and CI checks it against them.",
        "",
        "A message type absent here is one this client neither sends nor",
        "handles. That is not the same as one the venue never sends.",
        "",
        "## Sent",
        "",
    ]
    lines += table("Message types", out_msgs.keys(), TRADING)
    lines += [
        "### User-message subtypes",
        "",
        "A user message carries what it is for on tag 6040.",
        "",
        "| Subtype |", "| --- |",
    ] + [f"| `{s}` |" for s in out_subtypes] + [""]

    lines += ["## Handled", ""]
    lines += table("On the trading connection", inbound_arms(ccp, "match msg_type {"), TRADING)
    lines += [
        "### User-message subtypes handled",
        "",
        "| Subtype |", "| --- |",
    ] + [f"| `{s}` |" for s in in_subtypes] + [""]
    lines += table("On the market data connection", inbound_arms(farm, "match msg_type"), MARKET_DATA)

    OUT.write_text("\n".join(lines) + "\n")
    print(f"{OUT.relative_to(ROOT)} — {len(out_msgs)} sent, {len(out_subtypes)} subtypes sent")


if __name__ == "__main__":
    main()
