---
level: minor
changelog: A cron handler's scheduled time is an `Instant`, not an `Int`; `svc.schedule(expr, at)` takes an `Instant`
breaking: A cron handler declared `(at: Int)` is now `bynk.cron.bad_params`, and a test's `svc.schedule("…", 3600000)` address needs an `Instant`; write `(at: Instant)`, `Instant.fromEpochMillis(3600000)`, and `at.toEpochMillis()` where the body needs the number.
---

## ADR: cron-scheduled-time-is-instant
title: The cron scheduled time is an `Instant`
summary: `at: Int` becomes `bynk.cron.bad_params` now; the schedule test address takes an `Instant` only

**Context.** Cron (v0.10) predates `Instant` (v0.90, ADR 0114), so the scheduled
time was still handed to a handler as a bare epoch-millisecond `Int`, the last
place the language did that. Authors converted by hand or did timestamp maths on
an `Int`, which ADR 0114 made a `no_numeric_coercion` error everywhere else.

**Decision.** A. A cron handler's optional parameter must be `Instant`; `Int`
(and anything else) is `bynk.cron.bad_params`, with a note giving the
replacement and `at.toEpochMillis()` for the number. The language is pre-1.0 and
the fix is mechanical, so there is no deprecation period: keeping both would make
`Instant` optional where it should be the only spelling. B. The test address
`svc.schedule("<expr>", at)` takes an `Instant` only, because accepting `Int` as
well would keep the confusion in the one place authors write a literal time.
ADR 0522 D (the time stays required when the handler declares it; the epoch
otherwise) and ADR 0523 (it is not defaulted to the harness clock) stand. The
emitter and runtime are unchanged, since `Instant` erases to the same
epoch-millisecond `number`.

**Consequences.** Every cron handler declaring `at: Int` breaks, with a one-line
fix. The reference no longer says cron has no clock (`Clock.now()` exists) and
states that schedules run in UTC.
