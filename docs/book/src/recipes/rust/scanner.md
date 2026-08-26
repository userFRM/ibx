# Market Scanner

Run the `TOP_PERC_GAIN` scan over US major stocks, filtered to names above $5,
and print the top ten.

## What this shows

- `req_scanner_subscription(req_id, instrument, location_code, scan_code, max_items, filters)`.
- Filters as `TagValue` pairs. The tag names are the ones
  `req_scanner_parameters` publishes, for example `priceAbove` = `"5"` or
  `stkTypes` = `"inc:ETF"`. They are carried to the venue, not dropped.
- Reading `scanner_data` rows, each a rank and a `ContractDetails`, until
  `scanner_data_end`.
- `cancel_scanner_subscription(req_id)` before disconnecting.

## What comes back

One `scanner_data` callback per row, then `scanner_data_end` for the request id.
Rows are ranked; the example sorts by rank before printing because it collects
them into a `Vec` first.

A scanner subscription keeps answering until it is cancelled. The example
cancels; if you leave one running it goes on delivering into a session nobody is
reading.

## The shorter form

`scan` runs the scan, waits, and withdraws the subscription before returning:

```rust
let rows = client.scan("STK", "STK.US.MAJOR", "TOP_PERC_GAIN", 25)?;
```

It sends no filters. Use `req_scanner_subscription` when the scan needs them.

## Limits

Which scan codes and locations are valid is the venue's list.
`req_scanner_parameters` fetches it, and an unknown code comes back as a stated
refusal on the `error` callback.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... cargo run --example hello_scanner
```

## Source

```rust
{{#include ../../../../../examples/hello_scanner.rs}}
```
