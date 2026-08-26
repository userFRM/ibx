"""No way to fabricate a session reaches a caller who installed this.

`#[pyo3(...)]` publishes whatever it is put on. `#[doc(hidden)]` is a Rust
document attribute and never reaches the interpreter, and a leading underscore
hides nothing from `getattr`. So a method that pushes a fill, an account value
or a connected state into a client with no venue behind it is, once compiled
in, part of the published surface — `ibx.EClient(w)._test_connect("DU1")` and
the caller has a session that never spoke to anyone.

They are kept behind the `test-helpers` feature, which the suites ask for and
a wheel does not. This holds two things: that the module carrying them is
still gated, and that no second one has appeared outside it.

Exits non-zero on its own findings.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SURFACE = ROOT / "src/python"
GATED = "test_helpers"
FEATURE = 'feature = "test-helpers"'

problems: list[str] = []

# The module is declared behind the feature, and nothing has quietly removed it.
declaring = list(SURFACE.rglob("mod.rs"))
declared_at = [f for f in declaring if re.search(rf"^\s*mod {GATED};", f.read_text(), re.M)]
if not declared_at:
    problems.append(f"nothing declares `mod {GATED};` any more — has it been renamed?")
for f in declared_at:
    text = f.read_text()
    at = text.index(f"mod {GATED};")
    before = text[:at].rsplit("\n\n", 1)[-1]
    if FEATURE not in before:
        problems.append(
            f"{f.relative_to(ROOT)}: `mod {GATED};` is compiled into every build, so a "
            f"wheel carries the methods that fabricate a session"
        )

# And no other file on this surface publishes one.
for f in SURFACE.rglob("*.rs"):
    if f.stem == GATED:
        continue
    for line in f.read_text().splitlines():
        found = re.search(r"\bfn (_test_\w+)", line)
        if found:
            problems.append(
                f"{f.relative_to(ROOT)}: `{found.group(1)}` is published outside the "
                f"gated module, so a wheel carries it"
            )

if problems:
    print("\n".join(problems))
    print(
        f"\n{len(problems)} way(s) to fabricate a session reach a caller who installed "
        f"this. Move them behind the `test-helpers` feature."
    )
    sys.exit(1)

print("nothing that fabricates a session is compiled into a wheel")
