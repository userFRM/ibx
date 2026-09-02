"""Nothing here says what this client cannot have seen.

This repository is public. Its comments, its documentation and its commit
messages are read by people deciding whether to trust it with an account, and
every one of them is a claim.

Three kinds of claim have reached main and none of the checks beside this one
could see any of them:

The inside of something this client only speaks to over a socket. What the
vendor's own program keeps in memory, what it writes to disk, what it logs,
what it decides to grey out; what the venue holds, how it allocates, what it
charges. None of that crosses a wire. Where it is written down it was reasoned
and then set in the present tense.

A second factor. The account this was written against is a paper one, and a
paper session presents no second factor. Anything describing how one behaves —
how long it is allowed, the shape of the code, what a wrong one does — was
reasoned from the code and written as though it had been watched.

The vocabulary of how the work was done, rather than what the code does: a
round, an audit, a blocker, a verification pass. A reader wants the second and
is handed the first.

Exits non-zero on its own findings.
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: What a line must not say, and the sentence explaining why.
SAYINGS: list[tuple[str, re.Pattern[str], str]] = [
    (
        "describes the vendor's own program from the inside",
        re.compile(
            r"the (Java|vendor'?s?) client (reads|writes|keeps|logs|stores|persists|shows)"
            r"|vendor'?s own client (keeps|logs|stores|shows)",
            re.I,
        ),
        "nothing about the inside of that program crosses this wire, so nothing here "
        "can have seen it",
    ),
    (
        "states what the venue holds or charges",
        re.compile(
            r"\bis billed\b|\bbilled per\b|the venue (allocates|keys its|knows nothing)"
            r"|IB binds .* at first-login",
            re.I,
        ),
        "what the venue keeps internally and what an account is charged are not on "
        "this wire",
    ),
    (
        "describes a second factor as though it had been watched",
        re.compile(
            r"\b\d+[- ]char(acter)?s?\s+(?:[\w/]+[- ])?code\b"
            r"|\bcode\b[^\n]{0,20}\b\d+[- ]char"
            r"|one wrong code (ends|kills)"
            r"|server-side deadline fires \(~?\d",
            re.I,
        ),
        "the account this was written against is paper and presents no second factor, "
        "so this was reasoned rather than seen",
    ),
    (
        "narrates how the work was done",
        re.compile(
            r"\bcapture run [A-Z]\b|\brun-[A-Z] capture\b|\bround \d+\b"
            r"|\bverification pass\b|\bBLOCKER\b|\bproduction readiness\b",
            re.I,
        ),
        "a reader wants what the code does, not the sequence of attempts behind it",
    ),
]

#: Every kind of file a reader is handed. Workflows, manifests and notebooks
#: carry comments and prose the same as source does, and were never read.
PUBLISHED_IN = (".rs", ".py", ".md", ".yml", ".yaml", ".json", ".toml", ".ipynb")

#: The start of a line, up to its comment leader: what a claim written across
#: two comment lines has between its words.
LEADER = re.compile(r"^[ \t]*(?:///?|//!|#|\*|<!--)?[ \t]*")

#: Where a claim is published. Test fixtures and this file are exempt: a
#: fixture quotes the venue rather than speaking for the client, and this one
#: has to name what it forbids.
def published() -> list[pathlib.Path]:
    tracked = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.split()
    out = []
    for name in tracked:
        if name == "scripts/check_what_is_said.py":
            continue
        if name.endswith(PUBLISHED_IN) and not name.endswith("tests.rs"):
            out.append(pathlib.Path(name))
    return out


def main() -> int:
    problems: list[str] = []
    for name in published():
        try:
            body = (ROOT / name).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        lines = body.splitlines()
        for number, line in enumerate(lines, start=1):
            # This line and the start of the next, so a claim broken across a
            # line break is read as the sentence it is. Reported against the
            # line it starts on, once.
            following = LEADER.sub("", lines[number]) if number < len(lines) else ""
            window = f"{line} {following}"
            for what, pattern, why in SAYINGS:
                found = pattern.search(window)
                if found and found.start() <= len(line):
                    problems.append(f"{name}:{number} {what}: {window.strip()[:100]}\n    {why}")

    # A commit message is read on the same page as the code it changed, but
    # only for how it talks about the work. A commit that removes a claim has
    # to quote the claim, so the rules about what may be said are not applied
    # to it — a message saying "this said X and X was not observable" is the
    # fix, not the offence.
    log = subprocess.run(
        ["git", "log", "--format=%h%n%B%n--", "-40"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout
    narration = next(p for w, p, _ in SAYINGS if w == "narrates how the work was done")
    why_narration = next(r for w, _, r in SAYINGS if w == "narrates how the work was done")
    sha = ""
    for line in log.splitlines():
        if re.fullmatch(r"[0-9a-f]{7,40}", line):
            sha = line
            continue
        if narration.search(line):
            problems.append(
                f"commit {sha} narrates how the work was done: {line.strip()[:100]}"
                f"\n    {why_narration}"
            )

    if problems:
        print("\n".join(problems))
        print(f"\n{len(problems)} line(s) say what this client cannot have seen.")
        return 1
    print(f"{len(published())} published file(s) say only what this client can see")
    return 0


if __name__ == "__main__":
    sys.exit(main())
