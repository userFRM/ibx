"""Check that no module depends on one that depends on it.

A cycle between two modules means neither can be read, changed or tested
without the other. This crate had a seven-module cycle; it was untangled, and
then a compatibility shim put one edge back — a method returning a type no
caller can use, which cost the property and told nobody.

So the property is checked rather than remembered.
"""

import collections
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "src"


def module_of(path: pathlib.Path) -> str:
    """The top-level module a file belongs to."""
    return path.relative_to(SRC).parts[0].removesuffix(".rs")


def edges() -> dict[str, set[str]]:
    """What each module names, read from the code rather than its prose.

    Doc comments link across the whole crate on purpose — every module points
    at the public surface in its header — and a link is not a dependency.
    """
    out = collections.defaultdict(set)
    for f in sorted(SRC.rglob("*.rs")):
        if f.name == "tests.rs" or "/bin/" in str(f):
            continue
        body = "\n".join(
            l for l in f.read_text(errors="ignore").split("\n")
            if not l.lstrip().startswith(("//!", "///", "//"))
        )
        body = re.sub(r"#\[cfg\(test\)\]\s*mod tests \{.*", "", body, flags=re.S)
        here = module_of(f)
        for named in re.findall(r"\bcrate::(\w+)", body):
            if named != here and (SRC / named).exists() or (SRC / f"{named}.rs").exists():
                if named != here:
                    out[here].add(named)
    return out


def cycles(adj: dict[str, set[str]]) -> list[list[str]]:
    """Every group of modules that can reach each other. Tarjan's."""
    index, low, stack, on, counter, found = {}, {}, [], set(), [0], []

    def walk(v):
        index[v] = low[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on.add(v)
        for w in adj.get(v, ()):
            if w not in index:
                walk(w)
                low[v] = min(low[v], low[w])
            elif w in on:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            group = []
            while True:
                w = stack.pop()
                on.discard(w)
                group.append(w)
                if w == v:
                    break
            if len(group) > 1:
                found.append(sorted(group))

    sys.setrecursionlimit(10_000)
    for v in list(adj):
        if v not in index:
            walk(v)
    return found


def main() -> int:
    adj = edges()
    found = cycles(adj)
    if found:
        print("modules that depend on each other:")
        for group in found:
            print("  " + " ↔ ".join(group))
            for a in group:
                for b in sorted(adj[a] & set(group)):
                    print(f"    {a} names {b}")
        return 1
    print(f"{len(adj)} modules, no cycles")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
