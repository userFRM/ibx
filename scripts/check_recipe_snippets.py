"""Every Python snippet on the docs site names things that exist.

A recipe is read and trusted. One that calls a method the library does not
have, or spells an argument the way an older version spelled it, costs the
reader more than a missing page would: they assume their setup is wrong.

This does not run the snippets — they need a broker session. It parses each
one, then checks the names: every `ibx.Thing` the module actually exports, and
every method called on a client built from `ibx.IB()` or `ibx.EClient(...)`.
That is where the rot shows up first, because those are what a release renames.

Exits non-zero on its own findings.
"""

import ast
import importlib
import pathlib
import re
import sys

import ibx

ROOT = pathlib.Path(__file__).resolve().parent.parent
# The generated reference pages carry signatures rather than programs, so they
# are not snippets and do not parse as any.
GENERATED = {"python-reference.md", "rust-reference.md", "coverage-data.md"}
PAGES = sorted(
    p for p in (ROOT / "docs/book/src").rglob("*.md") if p.name not in GENERATED
)
FENCE = re.compile(r"```(?:python|py)\n(.*?)```", re.S)
INCLUDE = re.compile(r"\{\{#include ([^}]+)\}\}")


def resolved(block: str, page: pathlib.Path) -> str | None:
    """A snippet's real text.

    The pages hold the code by reference — mdBook pulls the file in when it
    builds — so what is on the page is a line naming a file. Checking that line
    checks nothing; the file is the snippet.
    """
    named = INCLUDE.search(block)
    if not named:
        return block
    target = (page.parent / named.group(1).strip()).resolve()
    if not target.exists():
        return None
    return target.read_text()

# What a client is built from, and what to check its methods against.
BUILDERS = {"IB": ibx.IB, "EClient": ibx.EClient, "Client": getattr(ibx, "Client", ibx.IB)}

problems: list[str] = []
checked = 0

for page in PAGES:
    for block in FENCE.findall(page.read_text()):
        checked += 1
        where = page.relative_to(ROOT)
        named = INCLUDE.search(block)
        text = resolved(block, page)
        if text is None:
            problems.append(
                f"{where}: includes {named.group(1).strip()}, which is not there"
            )
            continue
        if named:
            where = f"{where} -> {named.group(1).strip().split('/')[-1]}"
        try:
            tree = ast.parse(text)
        except SyntaxError as e:
            problems.append(f"{where}: a snippet does not parse: {e.msg} (line {e.lineno})")
            continue

        # Names bound to a client, so their method calls can be checked. A name
        # is only followed where it means one thing in the whole file: these
        # examples reuse short names, and `c` is a contract in one scope and a
        # client in another. Followed anyway, every field of the contract reads
        # as a method the client does not have.
        bound: dict[str, set] = {}
        for node in ast.walk(tree):
            if not isinstance(node, ast.Assign):
                continue
            made = None
            if isinstance(node.value, ast.Call):
                fn = node.value.func
                name = fn.attr if isinstance(fn, ast.Attribute) else getattr(fn, "id", "")
                made = BUILDERS.get(name, name or "?")
            else:
                made = "?"
            for t in node.targets:
                if isinstance(t, ast.Name):
                    bound.setdefault(t.id, set()).add(made)
        clients = {
            n: list(v)[0] for n, v in bound.items()
            if len(v) == 1 and list(v)[0] in BUILDERS.values()
        }

        for node in ast.walk(tree):
            if not isinstance(node, ast.Attribute):
                continue
            base = node.value
            if isinstance(base, ast.Name) and base.id == "ibx":
                if hasattr(ibx, node.attr):
                    continue
                # A submodule is not an attribute until something imports it.
                try:
                    importlib.import_module(f"ibx.{node.attr}")
                except ImportError:
                    problems.append(f"{where}: `ibx.{node.attr}` is not in the library")
            elif isinstance(base, ast.Name) and base.id in clients:
                owner = clients[base.id]
                if not hasattr(owner, node.attr):
                    problems.append(
                        f"{where}: `{base.id}.{node.attr}` is not on {owner.__name__}"
                    )

if problems:
    for p in sorted(set(problems)):
        print(p)
    print(f"\n{len(set(problems))} snippet(s) name something that does not exist.")
    sys.exit(1)

print(f"{checked} python snippet(s) across the site name only things that exist")
