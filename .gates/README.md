# Gate artifacts

Not documentation. Each file here is produced by a script under `scripts/` and
re-produced by continuous integration, which fails when what it produces
differs from what is committed. That is how the guarantees stated in the README
are held: a count cannot drift without the build noticing.

| File | Produced by | Holds |
| --- | --- | --- |
| `arg-reach.md` | `scripts/gen_arg_reach.py` | every argument a call takes is read |
| `order-field-reach.md` | `scripts/gen_order_field_reach.py` | no order field is silently dropped |
| `wire-reach.md` | `scripts/gen_wire_reach.py` | no call returns as though it acted when it did not |
