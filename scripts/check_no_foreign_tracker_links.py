"""Nothing here points at another project's issue tracker.

This repository is public and its examples are published verbatim on the docs
site. A line naming another tracker takes whoever reads it somewhere they
cannot go, and says that work happened somewhere they cannot see — the issue
numbers alone sketch a backlog. Four example files carried one, and one of the
four renders in full on the site.

An issue or pull-request link to this repository is fine: it is the same place
the reader already is. Anything else is not.

Exits non-zero on its own findings.
"""

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OURS = "userFRM/ibx"
LINK = re.compile(r"github\.com/([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+)/(?:issues|pull)/\d+")
# The numbering a foreign tracker leaves behind once its URL is gone.
NUMBERED = re.compile(r"^\s*(?:\"\"\"|#|//|/\*)?\s*Example #\d+\b")

tracked = subprocess.run(
    ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
).stdout.split()

problems: list[str] = []
for rel in tracked:
    path = ROOT / rel
    if not path.is_file():
        continue
    try:
        text = path.read_text()
    except (UnicodeDecodeError, OSError):
        continue
    for n, line in enumerate(text.splitlines(), 1):
        for repo in LINK.findall(line):
            if repo != OURS:
                problems.append(f"{rel}:{n}: names another project's tracker ({repo})")
        if NUMBERED.match(line):
            problems.append(f"{rel}:{n}: numbered from a tracker nobody reading this can open")

if problems:
    print("\n".join(problems))
    print(f"\n{len(problems)} line(s) point somewhere a reader cannot follow.")
    sys.exit(1)

print("nothing points at another project's tracker")
