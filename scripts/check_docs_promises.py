#!/usr/bin/env python3
"""What the book promises has to be here, and it must not name a door.

Three things the book can get wrong on its own, none of which the build
notices: it can include a file that is not there, it can tell a reader to run
an example that does not exist, and it can print a hostname nobody needs to
name — the session is told where it belongs by the venue, so a host written
into a recipe is at best noise and at worst a door that has moved.
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BOOK = ROOT / "docs" / "book" / "src"
EXAMPLES = ROOT / "examples"

# The doors this client knocks on live in one place, which is the source. A
# hostname anywhere a reader copies from is one this checker refuses.
DOORS = re.compile(r"\b[a-z]dc\d\.ibllc\.com\b")

INCLUDE = re.compile(r"\{\{#include\s+([^}\s]+)")
RUN_RUST = re.compile(r"cargo run --example (\S+)")
RUN_PY = re.compile(r"python3?\s+(examples/\S+\.py)")


def main() -> int:
    complaints: list[str] = []

    pages = sorted(BOOK.rglob("*.md"))
    if not pages:
        print(f"no pages under {BOOK}", file=sys.stderr)
        return 1

    for page in pages:
        text = page.read_text()
        where = page.relative_to(ROOT)

        for target in INCLUDE.findall(text):
            resolved = (page.parent / target).resolve()
            if not resolved.exists():
                complaints.append(f"{where}: includes {target}, which is not there")

        for name in RUN_RUST.findall(text):
            if not (EXAMPLES / f"{name}.rs").exists():
                complaints.append(f"{where}: says to run the example {name}, which is not there")

        for name in RUN_PY.findall(text):
            if not (ROOT / name).exists():
                complaints.append(f"{where}: says to run {name}, which is not there")

        for door in set(DOORS.findall(text)):
            complaints.append(f"{where}: names the door {door}; the venue names it instead")

    # And the same for what the pages include, which a reader copies whole.
    for source in sorted(list(EXAMPLES.rglob("*.rs")) + list(EXAMPLES.rglob("*.py"))):
        text = source.read_text()
        for door in set(DOORS.findall(text)):
            complaints.append(
                f"{source.relative_to(ROOT)}: names the door {door}; leave it unstated"
            )

    if complaints:
        print("The book promises what is not here:")
        for line in complaints:
            print(f"  {line}")
        print(f"\n{len(complaints)} to answer for.")
        return 1

    print(f"{len(pages)} pages: every include is there, every example named exists, no doors named")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
