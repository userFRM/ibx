"""Finding a module whichever file it lives in.

A module is `x.rs` until it grows a folder, and then it is `x/mod.rs`. A
generator that names one form stops reading the day it moves — and one that
reads nothing reports everything it was watching as absent, which is worse
than failing. Both `gen_wire_coverage` and `gen_order_field_reach` were caught
by this the day the engine's files were split.
"""

import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent


def module(name: str) -> pathlib.Path:
    """The file a module lives in, named without its extension.

    `module("src/gateway")` finds `src/gateway.rs` or `src/gateway/mod.rs`,
    whichever exists. Raises rather than returning a path to nothing: a
    generator reading an empty string reports the same as one reading a file
    with nothing in it, and only one of those is true.
    """
    flat = ROOT / f"{name}.rs"
    if flat.exists():
        return flat
    folded = ROOT / name / "mod.rs"
    if folded.exists():
        return folded
    raise FileNotFoundError(f"no module at {name}.rs or {name}/mod.rs")
