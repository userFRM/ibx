# Request Contract Details

Resolve a symbol: send a partial description, collect every contract the venue
lists under it, stop when the end marker lands. This is how you turn `"AAPL"`
into a `con_id` you can quote and trade on.

## What this shows

- Building a partial `Contract` from `symbol`, `sec_type`, `exchange` and
  `currency`.
- `req_contract_details(req_id, contract)` and the `contract_details` callback.
- Driving the callback loop on a daemon thread with `EClient.run`.
- Reading `con_id`, `primary_exchange` and `trading_class` off the returned
  `ContractDetails`.

## What comes back

One `contract_details` callback per matching contract, then one
`contract_details_end` for the request id. A description that matches several
listings answers with all of them, so more than one row means an ambiguous
description rather than an error.

## The shorter form

Two blocking methods do the same work with no wrapper and no callbacks:

```python
rows = c.contract_details(aapl)   # list, every match
one  = c.qualify_contract(aapl)   # a single Contract
```

`qualify_contract` refuses an ambiguous description rather than handing back
whichever the venue listed first, which would be a different contract from the
one you asked about. `c.qualify_contracts([...])` takes several at once.

These send, wait and return. They take their answer off the queue by request id
and release the interpreter lock while waiting, so they need no `run()` thread
of their own. Do not run both styles on one client: `run()` drains every queue
rather than only its own, so a dispatch loop beside a blocking call competes
with it for the answer. Pick the callbacks or pick these.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_contract_details.py
```

## Source

```python
{{#include ../../../../../examples/hello_contract_details.py}}
```
