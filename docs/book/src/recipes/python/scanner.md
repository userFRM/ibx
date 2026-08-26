# Market Scanner

Run the `TOP_PERC_GAIN` scan over US major stocks, filtered to names above $5,
and print the top ten.

## What this shows

- `req_scanner_subscription(req_id, subscription)`, where `subscription` is any
  object carrying the right attributes. The example defines a small class; the
  reference client's own `ScannerSubscription` works too.
- Reading `scanner_data` rows, each a rank and a `ContractDetails`, until
  `scanner_data_end`.
- `cancel_scanner_subscription(req_id)` before disconnecting.

## What the subscription object is read for

The scan itself:

| attribute | absent means |
|---|---|
| `instrument` | empty |
| `locationCode` | empty |
| `scanCode` | empty |
| `numberOfRows` | the venue picks |

Then the filters, each read by attribute and sent under the venue's own tag
name: `abovePrice`, `belowPrice`, `aboveVolume`, `marketCapAbove`,
`marketCapBelow`, `moodyRatingAbove`, `moodyRatingBelow`, `spRatingAbove`,
`spRatingBelow`, `maturityDateAbove`, `maturityDateBelow`, `couponRateAbove`,
`couponRateBelow`, `averageOptionVolumeAbove`, plus `excludeConvertible` and
`stockTypeFilter`.

An attribute you leave off takes its default. One that is present and cannot be
read is refused rather than run as a different scan under your request id.

## What comes back

One `scanner_data` callback per row, then `scanner_data_end` for the request id.
Rows are ranked; the example sorts by rank because it collects them into a list
first.

A scanner subscription keeps answering until it is cancelled. The example
cancels; if you leave one running it goes on delivering into a session nobody is
reading.

## Limits

Which scan codes and locations are valid is the venue's list.
`req_scanner_parameters()` fetches it, and an unknown code comes back as a
stated refusal on the `error` callback.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_scanner.py
```

## Source

```python
{{#include ../../../../../examples/hello_scanner.py}}
```
