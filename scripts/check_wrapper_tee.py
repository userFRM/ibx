#!/usr/bin/env python3
"""Every callback the Wrapper trait states must reach both of a Tee's sinks.

A question pumps the queues into its own collector, and the queues empty as
they are read. `Tee` is what also feeds the record a session keeps, so a
callback it does not forward is one that reaches the question and nobody else —
silently, because every trait method has a default empty body and a missing
forward compiles.

Checked here rather than left to review: the forwards are mechanical, there are
eighty-one of them, and a method added to the trait years from now is exactly
the case a reader stops checking.
"""
import re
import sys
from pathlib import Path

SRC = Path(__file__).resolve().parent.parent / "src" / "api" / "wrapper.rs"


def block(text: str, marker: str) -> str:
    """The braced body that follows `marker`."""
    i = text.index(marker)
    depth, j = 0, i
    while True:
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[i:j]
        j += 1


def signatures(body: str) -> dict[str, list[str]]:
    """Each `fn name(...)` in `body`, as its parameter names after self."""
    found: dict[str, list[str]] = {}
    for m in re.finditer(r"\n    fn (\w+)\(", body):
        name, k = m.group(1), m.end() - 1
        depth, start = 0, k
        while True:
            if body[k] == "(":
                depth += 1
            elif body[k] == ")":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        args, parts, nest, cur = body[start + 1:k], [], 0, ""
        for ch in args:
            if ch in "(<[":
                nest += 1
            elif ch in ")>]":
                nest -= 1
            if ch == "," and nest == 0:
                parts.append(cur.strip())
                cur = ""
            else:
                cur += ch
        if cur.strip():
            parts.append(cur.strip())
        found[name] = [p.split(":")[0].strip() for p in parts[1:]]
    return found


def main() -> int:
    src = SRC.read_text()
    trait = signatures(block(src, "pub trait Wrapper {"))
    tee_body = block(
        src, "impl<A: Wrapper + ?Sized, B: Wrapper + ?Sized> Wrapper for Tee<'_, A, B> {"
    )
    tee = signatures(tee_body)

    problems = []
    for name in sorted(set(trait) - set(tee)):
        problems.append(f"  {name} is stated on the trait and not forwarded")
    for name in sorted(set(tee) - set(trait)):
        problems.append(f"  {name} is forwarded and is not on the trait")

    for name, params in sorted(trait.items()):
        if name not in tee:
            continue
        if tee[name] != params:
            problems.append(f"  {name} takes {params} and forwards {tee[name]}")
            continue
        # And both sinks are called, with the parameters in the stated order.
        for sink in ("asked", "kept"):
            call = f"self.{sink}.{name}({', '.join(params)});"
            if call not in tee_body:
                problems.append(f"  {name} does not reach `{sink}` as `{call}`")

    if problems:
        print(f"the Tee does not carry every callback ({SRC}):")
        print("\n".join(problems))
        return 1
    print(f"Tee forwards all {len(trait)} Wrapper callbacks to both sinks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
