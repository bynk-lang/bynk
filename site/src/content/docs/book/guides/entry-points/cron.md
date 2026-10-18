---
title: Run a task on a schedule
---
**Goal:** run some work on a fixed schedule — a nightly sweep, an hourly
refresh — without an incoming request.

Cron handlers go in a `service` inside a `context`. Each names a schedule (bare,
after `cron`), takes at most one parameter, and returns `Effect[Result[(), E]]`.

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
minutes; `0 0 * * *` runs at midnight.

## Get the scheduled time

Cron has no built-in clock. To learn when the run fired, declare a single `Int`
parameter — it receives the scheduled time as Unix epoch milliseconds. It is the
exact, schedule-aligned instant (better than "now" for bucketing or idempotency
keys):

```bynk
context reaper

service sweeper from cron {
  on schedule("0 * * * *") (at: Int) -> Effect[Result[(), String]] {
    Ok(())
  }
}
```

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
scheduled time when the handler declares it. The address yields the handler's
`Result`:

```bynk,fragment
suite reaper as system {
  case "the hourly sweep succeeds" {
    let r <- sweeper.schedule("0 * * * *", 3600000)
    expect r is Ok(_)
  }
}
```

At `unit` and `integration` the handler runs in process. At `system` it runs
through the deployable Worker's own `scheduled` entry point, which needs a target
that already qualifies for `system` (an `http` or `queue` service, or a consumed
context). See [Cron and queue at `system`](/book/reference/testing/#cron-queue-system).

## Build and run

Cron services compile to a Cloudflare Worker with `--target workers`; each
schedule lands in the `[triggers]` table of the generated `wrangler.toml`. See
[Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/).

## Related

- Reference: [Cron](/book/reference/cron/).
- Reference: [HTTP](/book/reference/http/) — the sibling request-driven handler.
