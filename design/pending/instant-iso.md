---
level: minor
probes: ts_writes
changelog: "`Instant` reads and writes ISO-8601: `t.toIso()` gives RFC 3339 text in UTC (`YYYY-MM-DDTHH:mm:ss.sssZ`), and `Instant.fromIso(s)` reads the RFC 3339 date-time profile, converting an offset, as `Option[Instant]` (standard-library K3, [#2592](https://github.com/bynk-lang/bynk/issues/2592))"
---

## ADR: instant-iso
title: Instant reads and writes RFC 3339 text in UTC
summary: `toIso()` writes `YYYY-MM-DDTHH:mm:ss.sssZ` and `Instant.fromIso(s) -> Option[Instant]` reads the strict RFC 3339 date-time profile, offsets converted, fractions truncated, the calendar checked, through a hand-written recogniser in the shared runtime

**Context.** ADR 0114 made `Instant` a base type erased to epoch milliseconds,
with explicit conversions to and from `Int` (D6) and a JSON number codec (D7).
`toString()` gives the epoch number, so a service reading or writing a
timestamp in an HTTP API had to do RFC 3339 by hand. Track D7 (settled
2026-10-14) chose pure ISO-8601 parse and format in UTC, with time zones, DST
and calendar fields deferred. `Date.parse` accepts implementation-specific forms
that differ by engine, and `toISOString` throws past ±8.64e15 ms.

**Decision.**

- **A. Two kernel operations, named like `Bytes`'s hex pair.** `t.toIso() ->
  String` and `Instant.fromIso(s) -> Option[Instant]`. Bad input is ordinary
  data, so `fromIso` is `None`, not a fault. Both are pure, stable, and need
  no capability.
- **B. `toIso` has one shape.** `YYYY-MM-DDTHH:mm:ss.sssZ`, always three
  fraction digits and `Z`, so the text sorts in time order. Outside the years
  0000 to 9999 the year takes ISO 8601's expanded `±YYYYYY` form; past
  ±8.64e15 ms no date names the instant, so it faults.
- **C. `fromIso` reads the strict RFC 3339 date-time profile.** `T` or `t`, a
  fraction of any length (truncated toward the earlier instant past
  milliseconds), and `Z`, `z` or a `±HH:mm` offset, converted to UTC. The
  calendar is checked, leap years included. A leap second (`:60`) is `None`,
  since epoch milliseconds cannot name it. A space separator, a missing offset,
  the basic form, week and ordinal dates, and the expanded year are `None`.
  **Rejected:** reading a missing offset as UTC, which would assume a zone the
  text does not state.
- **D. A hand-written recogniser.** A regular expression for the shape, explicit
  range and calendar checks, then `Date.UTC` with the year set apart (it reads
  a year below 100 as 19xx), so every engine agrees.
- **E. In the shared runtime.** `__bynkInstantToIso` and `__bynkInstantFromIso`
  live in a new `time` runtime module, imported only by a module that calls
  them, so Cloudflare, Node and the browser behave identically.
- **F. Round trip.** `Instant.fromIso(t.toIso())` is `Some(t)` for every
  instant in the years 0000 to 9999; the runtime tests pin it over random
  samples.

**Consequences.** A service can accept and return RFC 3339 timestamps without
string handling of its own. `toString()` and the JSON codec are unchanged, so
nothing existing moves. Time zones and calendar fields remain deferred (D7).
