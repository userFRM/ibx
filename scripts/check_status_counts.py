#!/usr/bin/env python3
"""Check the test counts STATUS.md publishes against the tests that exist.

A number in a shipped document is a claim. Typed in once and left, it is a
claim that goes quietly wrong: the suites grow, the figure does not, and a
reader is told something that was true months ago. This counts what is there
and compares.

Counting, not running: `--list` and `--collect-only` name every test including
the ones that skip without credentials, which is what the table describes.

Prints what it found and exits non-zero on a disagreement.
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
STATUS = ROOT / "STATUS.md"

#: Row label in the table, and how to count what it describes.
ROWS = {
    "Rust unit and integration": "rust",
    "Python": "python",
    "Python, live": "python_live",
    "Paper compatibility suite": "paper",
}

#: How a phase of the paper suite names itself in its own output.
PHASE = re.compile(r"Phase \d+[a-z]?\b")


def _run(args: list[str]) -> str:
    done = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    return done.stdout


def _cargo_count(args: list[str]) -> int:
    """Tests named by one `--list` run, summed across its targets."""
    out = _run(["cargo", "test", *args, "--", "--list"])
    return sum(int(m) for m in re.findall(r"^(\d+) tests, \d+ benchmarks$", out, re.M))


def counted() -> dict[str, int]:
    # `--tests` names every target with tests in it — the library's own and
    # each integration target — so it is the whole of what the row describes.
    # The python feature adds the binding's tests to the same library, which
    # makes it the superset.
    rust = _cargo_count(["--tests", "--features", "python"])

    py = _run([".venv/bin/python", "-m", "pytest", "tests/python", "--collect-only", "-q"])
    collected = re.search(r"^(\d+) tests collected", py, re.M)
    python_all = int(collected.group(1)) if collected else 0

    # A live test is one that needs a session, which is to say one that reads
    # the credentials — not one with "live" in its name. The table names these
    # apart because a reader without credentials runs none of them.
    live_files = [p for p in sorted((ROOT / "tests" / "python").glob("*.py"))
                  if "IB_USERNAME" in p.read_text()]
    live = _run([".venv/bin/python", "-m", "pytest", *[str(p) for p in live_files],
                 "--collect-only", "-q"])
    live_hit = re.search(r"^(\d+) tests collected", live, re.M)
    python_live = int(live_hit.group(1)) if live_hit else 0

    return {
        "rust": rust,
        "python": python_all - python_live,
        "python_live": python_live,
        "paper": _cargo_count(["--test", "ib_paper_compat"]),
        "phases": len({
            m for f in (ROOT / "tests" / "ib_paper_compat").glob("*.rs")
            for m in PHASE.findall(f.read_text())
        }),
    }


def published() -> dict[str, int]:
    out: dict[str, int] = {}
    # The phase count rides in the paper suite's own row label.
    phases = re.search(r"Paper compatibility suite \((\d+) phases\)", STATUS.read_text())
    if phases:
        out["phases"] = int(phases.group(1))
    for line in STATUS.read_text().splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 2:
            continue
        # Longest label first: "Python, live" also starts with "Python", and
        # matching the shorter one would read one row's count into both.
        for label in sorted(ROWS, key=len, reverse=True):
            if cells[0].startswith(label) and ROWS[label] not in out:
                digits = re.sub(r"[^0-9]", "", cells[1])
                if digits:
                    out[ROWS[label]] = int(digits)
                break
    return out


def main() -> int:
    have, said = counted(), published()
    wrong = []
    for key, n in have.items():
        if key not in said:
            wrong.append(f"{key}: nothing published, {n} exist")
        elif said[key] != n:
            wrong.append(f"{key}: STATUS.md says {said[key]:,}, {n:,} exist")
        else:
            print(f"{key}: {n:,}")
    if wrong:
        print("\nSTATUS.md publishes a count that is not what is there:")
        for w in wrong:
            print(f"  {w}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
