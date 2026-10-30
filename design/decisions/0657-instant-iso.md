# 0657 — Instant reads and writes RFC 3339 text in UTC

- **Status:** Accepted (v0.443)

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
  milliseconds), and `Z`, `z` or a `±HH:mm` offset, converted to UTC.
  RFC 3339 §4.3 gives `-00:00` the meaning "the offset is unknown": the
  instant is still the UTC time written, so it reads as `Z` does. The
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
