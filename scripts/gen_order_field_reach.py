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

Writes .gates/order-field-reach.md. CI re-runs this and compares, so the number
cannot drift away from the code.
"""

import pathlib
import re
import sys

sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent))
from _paths import module, module_files  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / ".gates/order-field-reach.md"

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


def where_fields_are_read() -> str:
    """Everything that reads an order, with the order's own definition removed.

    `src/types/model.rs` both declares the fields and converts them for the
    engine. Searching it whole matches every field against its own
    declaration and reports that all of them are carried, which is the same
    blindness in the other direction; searching without it reports a field the
    conversion carries as lost.
    """
    out = []
    for path in BUILDERS:
        if not path.exists():
            continue
        text = path.read_text()
        # Whichever file holds it, not whichever file held it once: keyed on
        # a filename, this went quiet the day the model moved and reported
        # every field as carried by its own declaration. That is the fourth
        # time a moved file has silently disabled a check here.
        if "pub struct Order " in text:
            at = text.index("pub struct Order ")
            end = text.index("\n}", at)
            declaration = text[at:end]
            # And the value that fills it in, which names every field too.
            default = ""
            if "impl Default for Order" in text:
                d_at = text.index("impl Default for Order")
                default = text[d_at : text.index("\n}\n", d_at)]
            text = text.replace(declaration, "").replace(default, "")
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

    OUT.write_text("\n".join(lines))
    unstated = [f for f in constants_in_the_conversion() if f not in STATED_CONSTANTS]
    if unstated:
        print("the order conversion fills a field with a literal and nobody has")
        print("said why. A caller's value replaced by a constant reaches the venue")
        print("as though it were never set:")
        for f in unstated:
            print(f"  {f}")
        return 1

    print(f"{len(fields)} order fields: carried={len(carried)} "
          f"refused={len(says_so)} dropped={len(dropped)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
