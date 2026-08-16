#!/usr/bin/env python3
"""Count which fields of an order reach the venue.

`gen_wire_reach.py` counts calls. A call can reach the venue while most of what
a caller put in it does not: `place_order` sends an order, and a caller who set
a field the builder never reads has been told nothing about it. The order went
out without it, and the only sign is that the venue does something other than
what was asked.

Every field of `Order` falls into one of three kinds:

  carried   the order builder reads it and it goes out under a tag
  refused   this protocol does not carry it, and the field says so
  dropped   a caller can set it and nothing reads it

`dropped` is the one that matters. It is the order-field form of `silent`: the
call returns, the order is placed, and the field is gone.

Writes target/gates/order-field-reach.md, which is a report and not the gate:
this exits non-zero on its own findings. The number
cannot drift away from the code.
"""

import pathlib
import re
import sys

sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent))
from _paths import module, module_files  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "target/gates/order-field-reach.md"

# Where an order is turned into tags. A field that reaches the venue is read in
# one of these.
BUILDERS = [
    # Where a caller's order becomes the engine's, which is as much a part of
    # reaching the venue as the encoder is: a field the conversion drops never
    # gets as far as a tag. Leaving this out counted a carried field as lost.
    *module_files("src/types/model"),
    *module_files("src/engine/hot_loop/order_builder"),
    *module_files("src/engine/hot_loop/ccp"),
    *module_files("src/engine/hot_loop/mod"),
    *module_files("src/client_core"),
    *module_files("src/types"),
]


def order_fields() -> list[str]:
    text = module("src/types/model").read_text()
    at = text.index("pub struct Order ")
    end = text.index("\n}", at)
    return re.findall(r"^\s*pub (\w+):", text[at:end], re.M)


def refused() -> dict[str, str]:
    """Fields whose doc comment says this protocol does not carry them.

    Written on the field rather than kept in a list here, so that the reason
    sits where someone reading the field will find it.
    """
    text = module("src/types/model").read_text()
    at = text.index("pub struct Order ")
    end = text.index("\n}", at)
    out = {}
    block = text[at:end]
    for m in re.finditer(r"((?:^\s*///.*\n)+)\s*pub (\w+):", block, re.M):
        doc = " ".join(line.strip(" /") for line in m.group(1).strip().splitlines())
        if "not carried" in doc.lower():
            out[m.group(2)] = doc
    return out


def what_the_call_refuses() -> set[str]:
    """The fields `place_order` will not take a stated value for.

    The note on a field and the refusal in the call have to name the same set:
    a note without a refusal is an order that goes out missing what it says it
    cannot carry, and a refusal without a note is a caller told no with nowhere
    to read why.
    """
    text = module("src/client_core").read_text()
    at = text.index("refuse_if_stated!(")
    end = text.index(");", at)
    return set(re.findall(r"\b([a-z_]+)\b", text[at + len("refuse_if_stated!("):end]))


def without_tests(text: str) -> str:
    """The file with any `#[cfg(test)]` block after it removed.

    A test names the fields it exercises, and a name is all this looks for. A
    field read nowhere but by the test of the code that used to read it is a
    field nothing carries.
    """
    at = text.find("#[cfg(test)]")
    return text if at < 0 else text[:at]


def without_declarations(text: str) -> str:
    """The file with its struct definitions and their defaults removed.

    A declaration names every field and reads none. Left in, the attributes
    struct beside the builder said each name and the field counted as carried:
    a field could lose both its conversion and the tag it went out under and
    still be reported as reaching the venue, because two declarations of it
    remained.
    """
    out, at = [], 0
    while True:
        starts = [
            (text.find(k, at), k)
            for k in ("pub struct ", "impl Default for ", "struct ")
        ]
        starts = [(i, k) for i, k in starts if i >= 0]
        if not starts:
            out.append(text[at:])
            return "".join(out)
        start = min(starts)[0]
        end = text.find("\n}", start)
        if end < 0:
            out.append(text[at:])
            return "".join(out)
        out.append(text[at:start])
        at = end + 2


def the_conversion(text: str) -> str:
    """The order's conversion, and everything it calls to finish the job.

    In the file that declares the order, only the conversion carries a field to
    the engine. Everything else there names fields without carrying them — the
    declaration, the defaults, and a predicate asking whether an order has
    extended attributes at all, which names forty of them and would report
    every one as carried. `deactivate_on_disconnect` could lose both its
    conversion and its tag and stay counted, because that predicate still said
    its name.

    The conversion is not one function: it hands the scale, the delta hedge and
    the rest to helpers beside it. Taking only its own body reports every field
    those carry as dropped, so the calls are followed.
    """
    def body_of(name: str) -> str:
        at = text.find(f"fn {name}(")
        if at < 0:
            return ""
        end = text.find("\n    }\n", at)
        return text[at:end] if end > 0 else text[at:]

    wanted, seen, out = ["attrs"], set(), []
    while wanted:
        name = wanted.pop()
        if name in seen:
            continue
        seen.add(name)
        body = body_of(name)
        out.append(body)
        wanted += re.findall(r"self\.([a-z_0-9]+)\(", body)
    return "\n".join(out)


def where_fields_are_read() -> str:
    """Everything that reads an order, with the order's own definition removed.

    `src/types/model.rs` both declares the fields and converts them for the
    engine. Searching it whole matches every field against its own
    declaration and reports that all of them are carried, which is the same
    blindness in the other direction; searching without it reports a field the
    conversion carries as lost.

    The tests written beside the code are removed for the same reason the
    declaration is: a test naming a field is not code carrying it. Left in,
    a field whose only remaining mention was the test that used to check it
    read as carried — `deactivate_on_disconnect` could lose both its
    conversion and its tag and still be counted.
    """
    out = []
    for path in BUILDERS:
        if not path.exists():
            continue
        text = without_declarations(without_tests(path.read_text()))
        # Whichever file holds it, not whichever file held it once: keyed on
        # a filename, this went quiet the day the model moved and reported
        # every field as carried by its own declaration. That is the fourth
        # time a moved file has silently disabled a check here.
        if "pub fn attrs(&self)" in text:
            text = the_conversion(text)
        out.append(text)
    return "\n".join(out)


#: Fields the order conversion fills with a literal on purpose, and why.
#:
#: Anything else filled with a literal is a caller's value being replaced by a
#: constant. That is how `good_after_time` was lost: the caller set it, the
#: conversion wrote a zero over it, the builder declined to send a zero, and
#: the reach count called it carried because its name appeared in the builder.
#: Every step read correctly on its own.
STATED_CONSTANTS = {
    "primary_exchange": "carried on the contract, not on the order",
    "exercise_action": "set by the exercise call, not by an order",
}


def constants_in_the_conversion() -> list[str]:
    """Fields the conversion fills with a literal rather than the caller's value."""
    text = module("src/types/model").read_text()
    at = text.index("fn attrs(")
    end = text.index("\n    }", at)
    return [
        m.group(1)
        for m in re.finditer(
            r"^\s*(\w+): (\d+|false|true|String::new\(\)|Default::default\(\)),",
            text[at:end], re.M,
        )
    ]


def published(pattern: str) -> list[list[int]]:
    """The figures the capability matrix states, read back out of the sentence.

    A number in a shipped document is a claim. This one was stated once and
    then drifted, and the check that was supposed to hold it compared a
    generated report against its own committed copy — which is regenerated and
    committed by the same commit that moves the figure, so it never failed.
    Compare against the published prose instead.
    """
    text = (ROOT / "docs/capabilities.md").read_text()
    found = [
        [int(g.replace(",", "")) for g in m.groups()]
        for m in re.finditer(pattern, text)
    ]
    if not found:
        # A claim that has been reworded is a claim nobody is checking. Read as
        # "nothing published", this skipped silently and the figure it was
        # written to hold drifted anyway.
        raise SystemExit(
            f"docs/capabilities.md states nothing matching {pattern!r}, so the "
            f"figure it publishes is measured by nobody"
        )
    return found


def main() -> int:
    fields = order_fields()
    says_so = refused()
    read = where_fields_are_read()

    carried, dropped = [], []
    for field in fields:
        if field in says_so:
            continue
        if re.search(rf"\b{field}\b", read):
            carried.append(field)
        else:
            dropped.append(field)

    lines = [
        "# What of an order reaches the venue",
        "",
        "Generated by `scripts/gen_order_field_reach.py`. Do not edit.",
        "",
        "An order that reaches the venue is not thereby an order carrying what",
        "a caller put in it. A field the builder never reads is gone, and the",
        "only sign is the venue doing something other than what was asked.",
        "",
        "| Kind | Count | Meaning |",
        "| --- | ---: | --- |",
        f"| carried | {len(carried)} | goes out under a tag |",
        f"| refused | {len(says_so)} | this protocol does not carry it, and the field says so |",
        f"| dropped | {len(dropped)} | a caller can set it and nothing reads it |",
        "",
        "`dropped` is the order-field form of `silent`: the call returns, the",
        "order is placed, and the field is not on it.",
        "",
        "## Set by a caller and not read",
        "",
    ]
    lines.append(", ".join(f"`{f}`" for f in sorted(dropped)) if dropped else "None.")
    lines += ["", "## Not carried by this protocol", ""]
    lines.append(
        "\n".join(f"- `{f}` — {why}" for f, why in sorted(says_so.items()))
        if says_so
        else "None."
    )
    lines.append("")

    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(lines))

    if dropped:
        print("a caller can set these and nothing reads them. The call returns,")
        print("the order is placed, and the field is not on it:")
        for f in dropped:
            print(f"  {f}")
        return 1

    refuses = what_the_call_refuses()
    if refuses != set(says_so):
        print("a field's note and the call disagree about what can be placed:")
        for f in sorted(set(says_so) - refuses):
            print(f"  {f} is documented as not carried and the call takes it anyway")
        for f in sorted(refuses - set(says_so)):
            print(f"  {f} is refused by the call and no field says why")
        return 1

    unstated = [f for f in constants_in_the_conversion() if f not in STATED_CONSTANTS]
    if unstated:
        print("the order conversion fills a field with a literal and nobody has")
        print("said why. A caller's value replaced by a constant reaches the venue")
        print("as though it were never set:")
        for f in unstated:
            print(f"  {f}")
        return 1

    have = [len(fields), len(carried), len(says_so)]
    for pattern in (r"\| Order fields \| ([\d,]+)\. ([\d,]+) are sent; the other ([\d,]+) ",
                    r"An order has ([\d,]+) fields\. ([\d,]+) are sent\. The other ([\d,]+) "):
        for stated in published(pattern):
            if stated != have:
                print(f"docs/capabilities.md publishes {stated}, {have} exist")
                return 1
    print(f"{len(fields)} order fields: carried={len(carried)} "
          f"refused={len(says_so)} dropped={len(dropped)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
