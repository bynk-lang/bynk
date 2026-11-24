---
title: Unreleased
description: The increments merged since the latest Bynk release.
---
The increments merged since **v0.655.0**, the latest release. They ship in
the next one. Each increment's row is added here when it merges.

| Version | Highlights |
|---|---|
| **v0.661.1** | Fix `bynk check` panicking with "the events service has no recorded subscriber shape" when a test suite delivers an event with `Svc.event(e, env)` |
| **v0.661.0** | A `delivery { }` section on a `from queue` service sets the consumer's retries, retry delay, dead-letter queue, batching and concurrency in `wrangler.toml`; `bynk deploy` qualifies and creates the dead-letter queue; every queue consumer without a `deadLetter` now draws the warning `bynk.queue.no_dead_letter` (write `deadLetter: None` to say deleting is intended) |
| **v0.660.0** | `Matches` and the `String` regex methods run in Unicode mode (`u`), `Matches` accepts a leading `(?i)` or `(?s)` group, a refinement's `RegExp` is built once, and OpenAPI writes a `(?i)` pattern out or leaves it out with a note |
| **v0.659.0** | Routes are tried in a defined order (at the first differing segment a literal beats a `:name`), a route no request can reach is `bynk.http.unreachable_route`, and a literal route matches `/items/` and percent-decoded paths |
| **v0.658.0** | A cron run that returns `Err` or throws is marked failed in Cron Past Events and logs a labelled record naming the schedule, service, error type and variant (no payload); an unmatched `event.cron` warns |
| **v0.657.1** | A failing `for all x: Decimal` (or `Bytes`) prints its counterexample in Bynk's own form (`x = 0d`) instead of `[object Object]` |
| **v0.657.0** | `==`, `!=` and the ordering operators give a bare variant name that two sums share the other operand's sum as its expected type, so `e == Failed("down")` and `o == Some(Failed("x"))` resolve |
| **v0.656.0** | `<context>.<service>.outcome(args)` returns a `CallOutcome` (`Handled`, `Rejected`, `Faulted`, `Unreachable`), so a caller can handle a failed cross-context call; `bynk` gains `CallOutcome`, `CallFailure` and `Refusal`, and a consumed service's `stub` gains `rejected`, `faulted` and `unreachable` |
| **v0.655.2** | A virtual-time stub (`returns … after <d>`) at the `unit` tier no longer fails `tsc` with TS2559 |
| **v0.655.1** | A path with malformed percent-encoding is a `400 MalformedPath` rejection, no longer a `500` fault |
