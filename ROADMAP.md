# Roadmap

ibx is a Rust client that speaks to Interactive Brokers directly. There is no Java runtime, no desktop application, and no local gateway process to supervise. This document states what works today, what does not, and what has to be true before each milestone is called done.

Everything below is either measured against the code in this repository or observed against a live paper session. Where something is unverified, it says so. Nothing here is aspirational unless it appears under a milestone that has not shipped.

## How to read the status column

**Proven** means the path is implemented, covered by tests, and has passed against a live session.

**Implemented** means the path is built and covered by tests, but no live session has confirmed it. It is not a claim that it works against the venue.

**Partial** means the path is implemented and usable, but a stated part of it is missing or unproven.

**Accepted, not served** means the call exists with the expected signature and returns cleanly, and reports through the error callback that it cannot be served. It does not silently do nothing.

**Absent** means there is no such call.

## Where the client stands today

| Area | Status | Notes |
| --- | --- | --- |
| Connection, login, reconnect | Proven | Recovery across a dropped session is exercised live, including rebuilding subscriptions |
| Order placement | Proven | 23 order kinds, from market and limit through trailing, pegged, relative, snap, adaptive and algo |
| Order modification | Proven | A replace restates the order in full, so attributes survive it |
| Order cancellation | Proven | Individual, by permId, and global |
| Bracket and OCA orders | Proven | Parent and children linked on submission |
| Execution reports and fills | Proven | Includes replayed executions on reconnect. Busted and corrected trades are handled but have not been observed live, because none occurred |
| Account values | Proven | The account summary request has not been observed end to end |
| Market data, top of book | Proven | Including the frozen and delayed variants |
| Historical data | Proven | Bars, ticks, head timestamp, and schedules |
| Contract definitions | Proven | By identifier and by symbol |
| Scanners | Proven | Parameters and subscriptions |
| News | Proven | Providers, articles, historical news, and bulletins |
| Fundamental data | Proven | |
| Second factor approval | Implemented | Exercised for live logins. A paper login does not present the factor, so the paper suite does not reach it |
| Order conditions | Partial | Price, volume, percent and execution conditions are proven. A standalone time condition is refused by the venue, not by this client |
| Combination orders | Implemented | Legs carried with ratio, side, venue and position effect. No live coverage yet |
| Positions and round trip tracking | Implemented | The live phase depends on a fill and has not yet had one to observe |
| Market depth | Implemented | No depth updates observed live, which may be an entitlement on the account used |
| P&L subscription | Partial | Per contract P&L is proven. The account level subscription has not yet been observed end to end |
| Contract lookup by ISIN or CUSIP | Implemented | The identifiers are carried on the request. No live confirmation yet |
| Option chains | Absent | See below |
| Option exercise and lapse | Accepted, not served | |
| Server side option analytics | Accepted, not served | Implied volatility and option price |
| Wall Street Horizon event data | Accepted, not served | Requires a separate data subscription |
| Financial advisor allocation | Absent | Allocation groups and methods are not carried |
| Tick by tick data | Absent | The transport is not offered to this client at login |

### Asset classes

The contract layer names 24 security types, including equities, options, futures, futures options, forex, indices, bonds, warrants, funds, CFDs, commodities, crypto and combinations.

Order paths are proven live for equities, equity options and forex. Futures orders are refused by the venue as an ambiguous contract, and the cause has not been isolated. Index, bond and warrant order paths have no coverage yet.

A status of proven means a live phase passed against a paper session. A phase that skips because the market gave it nothing to observe, such as a position test waiting on a fill, leaves the path implemented rather than proven.

Instruments outside the United States resolve and stream correctly. Orders on them return an inactive state with no stated reason, which is consistent with the account lacking trading permission for those venues rather than with a defect in this client. Confirming that needs an account with those permissions enabled.

### What is measured

1126 tests cover the engine and the client surface. A further 308 cover the Python bindings. Every push runs both, along with lint, documentation, and builds for Linux, macOS and Windows.

The client surface is 92 methods in Rust and 123 in Python, against 125 callbacks.

Test count is not coverage. The figure above says what is checked, not what fraction of the venue's behaviour is reached.

## Milestones

### 0.8 Close the accepted but unserved calls

Option chains are the gap that matters most, because without them an option strategy cannot discover its own strikes and expirations. The definition query answers with a single contract by design, so the chain has to come from elsewhere. Until that is settled the call reports that it cannot be served rather than answering with part of a chain.

Exit criteria:

- Option chain discovery returns every expiration and strike a venue lists, for a named underlying
- Option exercise and lapse reach the venue
- Server side implied volatility and option price return values, or are documented as out of scope with the reason stated

### 0.9 Asset class completeness

Exit criteria:

- Futures orders accepted, with the ambiguity resolved and a regression test that would catch its return
- Index, bond and warrant orders proven against a live session
- Combination orders, market depth and account level P&L proven against a live session
- One venue outside the United States proven end to end for orders, not only for data
- Tick by tick data available, or documented as unavailable to this client with the reason stated

### 1.0 Behaviour a caller can rely on

Exit criteria:

- Every call either serves its request or reports through the error callback why it cannot. No call accepts a request and goes quiet
- The behaviour of a call before connection is settled and documented, and matches across the Rust and Python surfaces
- Financial advisor allocation carried, or declared a non goal
- A published compatibility statement listing every call, its status, and how that status was established

## Non goals

This is not a graphical application and will not become one. Features that exist to serve a screen, such as display groups, screen linkage and blotter state, are out of scope.

This is not a hosted service. It connects on behalf of the process that embeds it.

Order staging that exists only inside a desktop application, such as building an order without transmitting it, is out of scope. Where the venue itself holds an order until activation, that is carried, because the venue is the one holding it.

## Reporting a gap

If a call behaves differently from the venue's own client, that is a defect worth reporting, and the report is most useful when it states the call, what was expected, and what happened.
