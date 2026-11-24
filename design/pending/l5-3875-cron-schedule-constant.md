---
level: minor
changelog: A constant names a cron schedule: `on schedule(Nightly)` and `svc.schedule(Nightly)` accept a `String` or `CronSchedule` constant, resolved to its literal at compile time
---

## ADR: cron-schedule-constant
title: A constant names a cron schedule
summary: `schedule(…)` takes a `String` or `CronSchedule` constant; a refined constant is accepted for this position only

**Context.** A schedule shared by two places (a handler and a test, or two
contexts through a commons) had to be written twice. Constants exist for exactly
this (ADR 0678, 0826), but `schedule(…)` is a compile-time position they could not
reach. ADR 0826 A lets a constant stand in a literal only when declared exactly
the base type, which would refuse `const Nightly: CronSchedule`, the case most wanted.

**Decision.** A. `schedule(…)` accepts a string literal, a constant declared
exactly `String`, or a constant declared exactly `CronSchedule`; in every case the
chain resolves to a literal, which is then checked against `Cron`. A constant of
another refined type (`String where MaxLength(20)`, a user's own
`type Nightly = String where Cron`) is refused, keeping named types distinct
inside a context (ADR 0714 B). This extends ADR 0826 A from "exactly the base
type" to "exactly the type the position expects, which may be refined", for this
position only. B. ADR 0826 is not generalised: no annotation argument expects a
refined type today, so this ADR records the principle and the next position with a
refined type adopts it rather than deciding again. C. The test address accepts the
same forms and is matched to a handler by the resolved literal, so a test that
names the constant follows a schedule change. D. A refused constant is
`bynk.cron.invalid_schedule` with a note naming the constant and its declared
type (no new code, following ADR 0826 D); an unknown or private name is the
ordinary `unknown_name` / `bynk.visibility.private_item`. The name is resolved once,
upstream, in the annotation-constant pass (`annotation_consts`), which fills the
handler's schedule expression with the literal and keeps the reference
(`Handler::schedule_const`) so go-to-definition, find-references and rename see a
use; every later reader (the duplicate check, the trigger table, dispatch,
architecture facts) reads the resolved literal text.

**Consequences.** The `wrangler.toml` trigger table and the dispatch key stay
literal text. A commons cannot name the prelude's `CronSchedule` (it cannot name
any adapter type), so a schedule shared through a commons is a `String` constant;
`CronSchedule` constants live in a context that consumes `bynk`. Constants are
fixed at compile time; per-environment schedules are a separate question.
