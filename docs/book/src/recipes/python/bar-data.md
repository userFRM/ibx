# Historical Bars

Fetch one trading day of 5-minute SPY bars and print the first and last. Use
this whenever you need a finished series rather than a live feed.

## What this shows

- Building a `Contract` that carries `con_id`, so the venue is not asked to
  guess which listing you meant.
- `req_historical_data(req_id, contract, end_date_time, duration_str,
  bar_size_setting, what_to_show, use_rth)`, with `format_date`,
  `keep_up_to_date` and `chart_options` left at their defaults.
- Driving the callback loop on a daemon thread with `EClient.run`.
- Reading OHLCV off the bar object.

An empty `end_date_time` means now. `use_rth=1` keeps the series inside regular
hours.

## What comes back

One `historical_data` callback per bar, in time order, then one
`historical_data_end` carrying the first and last timestamps of the range that
was served.

With `keep_up_to_date=True` the bar still forming is folded in from the live
stream and keeps arriving. That bar opens on a whole multiple of its own length
counted from the epoch. Up to an hour that is the clock boundary you expect. At
`1 day` it is midnight UTC, so a US listing's forming daily bar opens in its
after-hours session and spans two of them.

## The shorter form

One blocking method does the same request and returns the list:

```python
bars = c.historical_data(spy, "", "1 D", "5 mins", "TRADES", use_rth=1)
```

It sends, waits and returns, taking its answer off the queue by request id and
releasing the interpreter lock while it waits. Do not run `run()` beside it on
the same client: `run()` drains every queue rather than only its own, so the two
compete for the answer. Pick the callbacks or pick this.

## Limits

`bar_size_setting` and `what_to_show` are checked before anything is sent, so a
misspelling is refused here rather than answered with a different series. With
`keep_up_to_date=True` the size must be one this client can form from
the five-second bars the venue keeps sending: five seconds up to a day, in
whole multiples of five seconds. A second is shorter than what arrives; a week
and a month would open where the venue's own never do.

How far back a series goes, and which bar sizes pair with which durations, are
the venue's rules. A request outside them comes back as a stated refusal on the
`error` callback rather than as empty bars.

## Run it

```bash
IB_USERNAME=... IB_PASSWORD=... python examples/hello_bar_data.py
```

## Source

```python
{{#include ../../../../../examples/hello_bar_data.py}}
```
