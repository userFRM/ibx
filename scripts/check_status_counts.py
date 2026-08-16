#!/usr/bin/env python3
"""Check the counts docs/capabilities.md publishes against what exists.

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
# The inventory sits with the engineering notes: it is a fact about this
# repository rather than about what the client can do, and the compatibility
# matrix is the latter.
STATUS = ROOT / "docs" / "engineering-notes.md"
MATRIX = ROOT / "docs/capabilities.md"

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


def _python() -> str:
    """An interpreter that can collect the Python suite.

    The virtual environment where one exists, and whatever is running this
    otherwise: a checker that only works on a machine set up one particular
    way fails on the machine that matters, which is the one that decides
    whether a commit lands.
    """
    venv = ROOT / ".venv" / "bin" / "python"
    for candidate in (str(venv), sys.executable):
        if candidate == str(venv) and not venv.exists():
            continue
        probe = subprocess.run(
            [candidate, "-c", "import pytest"], cwd=ROOT, capture_output=True,
        )
        if probe.returncode == 0:
            return candidate
    raise SystemExit(
        "no interpreter here can import pytest, so the Python suite cannot be "
        "counted. A count nobody can take is not a count that agrees."
    )


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

    python = _python()
    py = _run([python, "-m", "pytest", "tests/python", "--collect-only", "-q"])
    collected = re.search(r"^(\d+) tests collected", py, re.M)
    python_all = int(collected.group(1)) if collected else 0

    # A live test is one that needs a session, which is to say one that reads
    # the credentials — not one with "live" in its name. The table names these
    # apart because a reader without credentials runs none of them.
    live_files = [p for p in sorted((ROOT / "tests" / "python").glob("*.py"))
                  if "IB_USERNAME" in p.read_text()]
    live = _run([python, "-m", "pytest", *[str(p) for p in live_files],
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


def capabilities() -> tuple[int, int]:
    """How many capabilities the matrix lists, and how many are verified."""
    text = (ROOT / "docs/capabilities.md").read_text()
    # From the first capability section, so the table defining the marks is
    # not counted as capabilities carrying them.
    rows = text[text.index("## Client surfaces"):].splitlines()
    verified = sum(1 for l in rows if l.startswith("| ") and "| \u2705 Supported |" in l)
    other = sum(
        1 for l in rows
        if l.startswith("| ") and ("| \U0001f52c Implemented |" in l or "| \u26d4" in l)
    )
    return verified, verified + other


def readme_says() -> tuple[int, int] | None:
    """What the matrix's own test row states, offline and session-only.

    Both figures count tests as written, not as run: the second set is skipped
    without credentials, so it is what would run against a session and not a
    record of what has.

    The row is prose rather than a table of counts, so it is read back out of
    the sentence it is written in. A figure nobody re-derives is one that goes
    stale quietly — this one was two hundred tests out before anything checked
    it.
    """
    m = re.search(r"\| ([\d,]+) offline, and ([\d,]+) more that only run against a broker session \|",
                  MATRIX.read_text())
    if not m:
        return None
    return (int(m.group(1).replace(",", "")), int(m.group(2).replace(",", "")))


def main() -> int:
    have, said = counted(), published()
    wrong = []

    # The prose states the same thing in a sentence.
    offline = have["rust"] + have["python"]
    live = have["python_live"] + have["paper"]
    verified, total = capabilities()
    # The prose above the matrix restates what the matrix lists. Both live in
    # the same file now, which is not a reason to trust one against the other:
    # the sentence is typed and the marks are counted.
    matrix_text = MATRIX.read_text()
    if f"{verified} of the {total} capabilities" not in matrix_text:
        wrong.append(
            f"docs/capabilities.md does not say {verified} of {total} "
            f"capabilities, which is what its own matrix lists"
        )
    else:
        print(f"capabilities: {verified} of {total} verified")

    stated = readme_says()
    if stated is None:
        wrong.append("docs/capabilities.md states no test counts")
    elif stated != (offline, live):
        wrong.append(
            f"docs/capabilities.md says {stated[0]:,} offline and {stated[1]:,} live, "
            f"{offline:,} and {live:,} exist"
        )
    else:
        print(f"readme: {offline:,} offline, {live:,} live")

    for key, n in have.items():
        if key not in said:
            wrong.append(f"{key}: nothing published, {n} exist")
        elif said[key] != n:
            wrong.append(f"{key}: docs/engineering-notes.md says {said[key]:,}, {n:,} exist")
        else:
            print(f"{key}: {n:,}")
    if wrong:
        print("\nA published count is not what is there:")
        for w in wrong:
            print(f"  {w}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
