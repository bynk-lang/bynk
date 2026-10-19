---
title: Testing against a running system
---
**Goal:** run the `system` suites you already have against a real runtime,
first a local `bynk dev` and then a preview deployment, without changing a
case.

In process, a `system` suite stands each context up as the Worker it deploys
as and crosses the real serialisation edge. What it can't show is the
platform itself: the real Workers runtime, real Durable Objects, real
bindings. `bynk test --against` sends the same cases' requests to Workers
that are actually running.

## 1. Against a local `bynk dev`

```sh
bynk test --against dev
```

The runner builds the project, starts a local [`bynk dev`](/docs/bynk-cli/#bynk-dev)
for the run, waits for it, sends each `system` case's requests to it, and stops
it. Unit and `integration` suites still run in process. Nothing reaches
Cloudflare: `bynk dev` runs `wrangler dev` locally.

A case's [`env`](/book/reference/testing/#env) is applied to the Workers, so
cases with different `env` maps run against separate `bynk dev` sessions, one
after another. See
[Running against `bynk dev`](/docs/bynk-cli/#running-against-bynk-dev).

## 2. Against a preview deployment

Name the deployment under `[test.targets]` in `bynk.toml`:

```toml
[test.targets.preview]
url.shop-orders = "https://shop-orders-preview.example.workers.dev"
secrets = ["AUTH_JWT_SECRET"]
vars = { PAYMENTS_PROCESSOR = "fake" }
```

Then run against it, with each listed secret in the environment:

```sh
AUTH_JWT_SECRET=… bynk test --against preview
```

The runner prints the target and each Worker's URL before the first request.
Three guards keep a run off production:

- `--against` takes only `dev` or a name from `[test.targets]`, never a URL;
- a target marked `production = true` is refused;
- a case's `env` must agree with the target's `vars`, so a suite written for a
  fake payment processor can't run against one wired to the real one.

See [`[test.targets]`](/docs/manifest/#test) and
[Running against a deployment](/docs/bynk-cli/#running-against-a-deployment).

## 3. Write cases that hold on a real system

A real system differs from the in-process harness in three ways. Each has a
tool.

**State persists.** A deployment keeps what earlier runs wrote. Key what a case
creates by [`run.id`](/book/reference/testing/#run-id), fresh for each
`--against` run and `"local"` in process:

```bynk,fragment
case "a new order is accepted" {
  let r <- api.POST("/orders/:id", "order-\(run.id)") by Visitor
  expect r is Ok(true)
}
```

**Writes land a moment later.** A failing `expect` is retried, replaying the
safe reads before it, until it holds or `--settle` expires (default `5s`, for
each retried stretch rather than the whole case). A `POST` is never replayed,
and an `expect` after a `let … else` isn't retried. Each case's retry count is reported, so an ordering
bug shows up as retries rather than hiding behind a pass. See
[Waiting for a real system](/docs/bynk-cli/#waiting-for-a-real-system).

**The clock is real.** The Workers read wall-clock time, which the runner
can't move: `advance Clock` is refused, and an `expect` comparing an `Instant`
built from a constant draws a `bynk.test.remote_clock_dependent` warning.
Compare instants the run produced (`expires > created`) instead of absolute
ones.

## 4. What can't run remotely

A deployment answers only on its public `http` routes. A case that moves the
clock, addresses an `on call` service, delivers a queue message, runs a cron
handler, or fetches an asset is refused with a `bynk.test.remote_unsupported_*`
diagnostic. Add `--skip-unsupported` to skip such cases instead; each is named,
with the reason, and counted as skipped.

The in-process run stays the gate: it is deterministic, fast, and checks
everything. Run against a real system to catch what only the platform can
show.
