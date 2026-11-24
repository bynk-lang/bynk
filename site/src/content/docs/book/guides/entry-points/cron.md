---
title: Run a task on a schedule
---
**Goal:** run some work on a fixed schedule — a nightly sweep, an hourly
refresh — without an incoming request.

Cron handlers go in a `service` inside a `context`. Each is an `on schedule("…")`, takes at most one parameter, and returns `Effect[Result[(), E]]`.

## A minimal scheduled task

```bynk
context reaper

service sweeper from cron {
	on schedule("*/5 * * * *") () -> Effect[Result[(), String]] {
		Ok(())
	}
}
```

The schedule is a standard five-field cron expression
(`minute hour day-of-month month day-of-week`). `*/5 * * * *` runs every five
minutes; `0 0 * * *` runs at midnight. Schedules run in UTC, fields are separated
by exactly one space, and the day of week counts `1` as Sunday (write `MON-FRI`
rather than numbers). The compiler checks every field, so `61 * * * *` is
`bynk.cron.invalid_schedule`; see [the syntax](/reference/cron/#schedule-syntax).
The type `CronSchedule` names a valid schedule.

## Get the scheduled time

To learn when the run fired, declare a single `Instant` parameter — it receives
the scheduled time. It is the exact, schedule-aligned instant (better than
`Clock.now()` for bucketing or idempotency keys, because it stays the same when a
run starts late), and it works directly with `Duration` arithmetic such as
`at - 1.hour`. Schedules run in UTC. Where you need the number, use
`at.toEpochMillis()`:

```bynk
context reaper

service sweeper from cron {
	on schedule("0 * * * *") (at: Instant) -> Effect[Result[(), String]] {
		Ok(())
	}
}
```

## Share a schedule by name

Write a schedule once with a constant and name it in `schedule(…)`:

```bynk,fragment
const Nightly: String = "0 2 * * *"

service sweeper from cron {
	on schedule(Nightly) () -> Effect[Result[(), String]] {
		Ok(())
	}
}
```

The name resolves to its literal at compile time, so the generated
`wrangler.toml` still lists the literal text. Put the constant in a commons to
share it between contexts, and name it in a test (`sweeper.schedule(Nightly)`) so
the test follows a change to the schedule. See
[Cron](/reference/cron/#naming-a-schedule-with-a-constant).

## Signal success or failure

Return `Ok(())` when the run succeeds. A cron run has no caller to answer and no
retry, so a failure is returned as `Err(e)` — it is logged and the run
completes. Map a domain error to `Err` explicitly:

```bynk
type SweepError = enum { StorageUnavailable }

service sweeper from cron {
  on schedule("0 0 * * *") () -> Effect[Result[(), SweepError]] {
    Err(StorageUnavailable)
  }
}
```

## Test it

A test case runs a cron handler with `svc.schedule("<expr>", at)`, passing the
scheduled time (an `Instant`) when the handler declares it. The address yields the handler's
`Result`:

```bynk,fragment
suite reaper as system {
	case "the hourly sweep succeeds" {
		let r <- sweeper.schedule("0 * * * *", Instant.fromEpochMillis(3600000))
		expect r is Ok(_)
	}
}
```

At `unit` and `integration` the handler runs in process. At `system` it runs
through the deployable Worker's own `scheduled` entry point, which needs a target
that already qualifies for `system` (an `http` or `queue` service, or a consumed
context). See [Cron and queue at `system`](/reference/testing/#cron-queue-system).

## Build and run

Cron services compile to a Cloudflare Worker with `--target workers`; each
schedule lands in the `[triggers]` table of the generated `wrangler.toml`. See
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).

## Related

- Reference: [Cron](/reference/cron/).
- Reference: [HTTP](/reference/http/) — the sibling request-driven handler.
