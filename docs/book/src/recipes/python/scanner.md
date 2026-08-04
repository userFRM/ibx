# Market Scanner

Subscribe to the `TOP_PERC_GAIN` scanner over US major stocks and print the top 10 results.

## What this shows

- Building a `ScannerSubscription`-shaped object (IBX reads attributes via `getattr`), including its filter attributes such as `abovePrice`.
- Reading `scanner_data` rows (rank + `ContractDetails`) until `scanner_data_end`.
- Cancelling cleanly with `cancel_scanner_subscription` before disconnecting.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_scanner.py
```

## Source

```python
{{#include ../../../../../examples/hello_scanner.py}}
```
