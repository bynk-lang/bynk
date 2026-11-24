---
title: Cron
---
Cron handlers run on a schedule. Like HTTP handlers, they are declared in a
`service` inside a `context`; the protocol sits on the service header
(`from cron`), and each handler is an `on schedule("…")`.

## Handler form

```bynk,ignore
service <Name> from cron {
  on schedule("<schedule>") (at: Instant) -> Effect[Result[(), E]] {
    …
  }
}
```

- **Schedule:** a cron expression as a string literal in `schedule("…")`, in
  exactly Cloudflare's syntax (see [Schedule syntax](#schedule-syntax)). An
  invalid one is `bynk.cron.invalid_schedule`, and the message names the failing
  field. Schedules run in **UTC**.
- **Parameter:** *optional*, at most one. If present it must be `Instant` and
  receives the scheduled fire time; otherwise `bynk.cron.bad_params`. An `Int`
  parameter, the spelling before v0.90's `Instant` reached cron, is that error
  too: write `at: Instant`, and `at.toEpochMillis()` where the body needs the
  number. For "now", use `Clock.now()`; the parameter is the exact,
  schedule-aligned instant, which stays the same when a run starts late (useful
  for bucketing and idempotency). It works directly with `Duration` arithmetic
  (`at - 1.hour`) and `Instant` comparison.
- **Return type:** must be `Effect[Result[(), E]]` for some error type `E`
  (`bynk.cron.return_not_effect_result`).
- **Placement:** only inside a `service`, never an `agent`
  (`bynk.parse.handler_in_agent`).

No two cron handlers in a context may declare the same schedule
(`bynk.cron.duplicate_schedule`).

## Schedule syntax

Five fields, separated by **exactly one space**, with no leading or trailing
space. The string is then exactly what Cloudflare hands the Worker as
`event.cron`, which it matches character for character, so nothing is
normalised.

| Field | Values | Also |
|---|---|---|
| minute | `0-59` | |
| hour | `0-23` | |
| day of month | `1-31` | `L` (last day), `W` (weekday nearest: `15W`, `LW`) |
| month | `1-12` or `JAN`-`DEC` | |
| day of week | `1-7`, with **`1` = Sunday**, or `SUN`-`SAT` | `6L` or `FRIL` (the last such weekday of the month), `6#2` (the second) |

Names are case-insensitive, and `*`, `,`, `-` and `/` are valid in every field
(`*/5`, `0-30/5`, `MON-FRI`, `1,15`). A range runs low to high, and a step is at
least 1. There are no seconds, no year, no `?` and no `@daily`-style macros.
Examples: `0 18 * * 6L`, `0 18 * * friL`, `59 23 LW * *`. Schedules run in
**UTC**. `*/45 * * * *` fires at minute 0 and 45 of each hour.

Where Cloudflare's documentation does not settle a form (a bare `L` in the day of
week, a step on a single value), it is refused; a refusal can be relaxed later.

Two warnings help with the platform's habits, and either is silenced by writing
a name or an explicit list: `bynk.cron.numeric_weekday` (`5` is Thursday here,
not standard cron's Friday; write `FRI`) and `bynk.cron.uneven_step` (a `*/n` in
the minute or hour field where `n` doesn't divide 60 or 24, so the last gap is
shorter).

The same rule is the [`Cron` refinement predicate](/reference/refined-types/), and
`CronSchedule` (`String where Cron`, from the `bynk` adapter) is the type of a
valid schedule: `let s: CronSchedule = "0 2 * * MON-FRI"` is checked at compile
time.

## Failure

A cron run has no retry channel. Returning `Ok(())` completes the run silently;
returning `Err(e)` logs `e` and completes. Map a domain failure to `Err` the
same way an HTTP handler maps it to an `HttpResult` error variant — the mapping
stays visible in the handler body.

## Example

```bynk
context reaper

service sweeper from cron {
	on schedule("*/5 * * * *") (at: Instant) -> Effect[Result[(), String]] {
		Ok(())
	}
}
```

## Emission

cron handlers compile to the Worker's `scheduled` entry point on the
`--target workers` target (dispatching on `event.cron`, passing
`event.scheduledTime` to handlers that declare the parameter), and every
schedule is aggregated into the `[triggers]` table of the generated
`wrangler.toml`. See [emission](/reference/tooling/emission/) and
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).
