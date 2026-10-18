---
title: "Run your project locally with `bynk dev`"
---
**Goal:** build your project and serve it on a local URL in one step — no
compile flags to remember, no `cd` into a generated directory, no Cloudflare
account.

```sh
bynk dev          # from anywhere inside the project
```

That's the whole thing. `bynk dev` finds your project root (the nearest
`bynk.toml`), compiles it to Workers, and runs
[`wrangler dev`](https://developers.cloudflare.com/workers/wrangler/) for you —
one per context, with the service bindings between them wired. Your service comes
up on `http://localhost:8787`.

> **Understand — local dev needs no provisioning.** `wrangler dev` runs in
> *local mode* (Miniflare), which simulates KV, Durable Objects, and queues
> keyed by **binding name**. The `id = "<KV_NAMESPACE_ID>"` placeholders in the
> generated `wrangler.toml` are only read when you deploy for real — local mode
> ignores them. So a KV- or agent-backed project runs locally against the
> generated config untouched; there is no namespace to create first.

## What it does

`bynk dev` collapses the manual recipe (compile → `cd` → `wrangler dev`) into
one command. In order, it:

1. **Locates the project** — walks up for `bynk.toml` and reads your `[paths]
   include`/`exclude` layout, so you can run it from any subdirectory.
2. **Pre-flights** — checks that `bynkc`, Node, and `wrangler` are usable, with
   the same report (and fix-it lines) as [`bynk
   doctor`](/docs/editor-and-tooling/doctor/). A missing tool fails here, before
   anything is built.
3. **Compiles** — the same project shape as `bynkc compile <project-root>
   --target workers`, into a managed build directory (see [The build
   directory](#the-build-directory) below).
4. **Selects the workers** — every context, unless you narrow with
   `--context`; see [Multi-context projects](#multi-context-projects).
5. **Serves** — runs one `wrangler dev` per context, from inside its worker
   directory, and wires the service bindings between them.
6. **Watches** — while serving, your `.bynk` sources are watched: saving a
   file rebuilds in place and the running workers hot-reload. A rebuild that
   fails type-checking reports the errors and keeps serving the last good
   build — fix the file and save again.

`bynkc` type-checks as part of compiling, so a type error stops you here with
the usual diagnostics — there is no separate `check` step to run.

## The build directory

`bynk dev` compiles into a driver-managed **`.bynk/dev/`** under your project
root — the same relationship `cargo`'s `target/` has to your source. It is
created and gitignored automatically (a `.bynk/.gitignore` containing `*` is
written on first build), so a `dev` run never dirties `git status` and you never
edit your own ignore file. Your own `bynkc compile --output out` builds, if you
keep any, are left alone — `out/` stays yours.

## Multi-context projects

A project with several contexts compiles to several workers, and `bynk dev`
serves **all of them**, with the service bindings between them wired. A
cross-context call — `Payment.authorise(total)` from your orders context —
resolves against the payment worker actually running next to it:

```sh
bynk dev
```

```
bynk dev: serving 2 contexts — service bindings between them are wired.
  commerce-orders   http://localhost:8787
  commerce-payment  http://localhost:8788
```

Each context gets its own port, allocated from `--base-port` (8787 by default) in
the order listed. Every context is addressable, so you can exercise one directly
or call it through another.

> **Understand — why all of them, and not one.** A `consumes` relationship
> compiles to a Cloudflare [Service
> Binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/),
> and a binding only resolves when the worker it points at is up too. Serving one
> context of a multi-context project would leave every cross-context call
> dangling — so "several contexts" isn't an ambiguity to resolve, it's the shape
> your architecture is meant to have.

To run fewer workers, narrow with `--context`. It's repeatable, and accepts the
context name in either form (`commerce.payment` or its worker-directory form
`commerce-payment`):

```sh
bynk dev --context commerce.payment                        # just one
bynk dev --context commerce.orders --context commerce.payment  # a chosen pair
```

A context you leave out simply isn't running: calls into it will fail, exactly as
they would if you'd stopped it.

## Passing options through to wrangler

`bynk dev` owns the flags that are its own concepts — `--context`, and the port
allocation (`--base-port`, `--inspect-port`) — and forwards everything after `--`
to `wrangler dev` verbatim, so it stays stable as wrangler evolves:

```sh
bynk dev --base-port 9000                     # move the whole allocation
bynk dev -- --var AUTH_JWT_SECRET:dev-secret  # supply a local secret
bynk dev -- --persist-to .wrangler-state      # control where local state lives
```

Ports are the one exception to the passthrough. Each worker needs a port of its
own, so `bynk dev` allocates them and `--base-port` moves the block; passing
`-- --port` yourself is an error that tells you so. (A single-context project
left on the default port is unaffected: `bynk dev -- --port 8788` still works
there, because there's nothing to allocate.)

If your service reads secrets (a `Bearer` actor's `AUTH_JWT_SECRET`, a webhook
`WEBHOOK_SECRET`, …), pass them with `-- --var KEY:VALUE` for local runs — you
don't need real Cloudflare secrets to develop.

> Local KV / Durable Object state persists under `.wrangler/` between runs.
> That's usually what you want; clear that directory (or point `--persist-to`
> elsewhere) for a clean slate.

## Debugging the workers (`--inspect`)

`bynk dev --inspect` serves with the V8 inspector enabled, so you can attach a
JavaScript debugger and set breakpoints **in your `.bynk` source**:

```sh
bynk dev --inspect                 # inspector on port 9229
bynk dev --inspect --inspect-port 9300
```

Each context gets its own inspector port, allocated from `--inspect-port` just as
the HTTP ports are allocated from `--base-port` — so in a two-context project the
inspectors land on 9229 and 9230, and you attach to whichever context you want to
break in. Narrow with `--context` if you'd rather debug one alone.

It prints an inspector URL per context on start. Attach any CDP client — VS Code's built-in
JavaScript debugger, Chrome DevTools — and breakpoints set in `.bynk` bind and
pause on real requests: the compiler emits source maps (since v0.68, per-statement
in handler bodies since v0.70), and `wrangler`/esbuild composes them into the
worker bundle, so the debugger resolves the running code back to your `.bynk`
lines.

> One wrinkle: `wrangler`'s inspector requires an `Origin` header on the
> WebSocket connection. VS Code's debugger sends one automatically; a hand-rolled
> CDP client must set it (`Origin: http://localhost`), or the connection is
> rejected with `400 Bad Request`.

## From VS Code

The [VS Code extension](/docs/tooling/vscode-bynk/) runs `bynk dev` for you.
**Bynk: Start Dev Server** serves the project in a terminal, and asks which
context to serve when there are several. A status-bar item then shows each
context's URL; click it to open one in the browser or to stop the server. The
same server is the `bynk: dev` task, so a launch configuration can start it as
its `preLaunchTask`.

A script or another tool can follow the same session with `bynk dev --format
json`, which writes one [lifecycle event](/docs/bynk-cli/#lifecycle-events) per
line: the ports each context got, and each rebuild's outcome.

## When `wrangler` isn't installed

`bynk dev` resolves `wrangler` the same way `doctor` does: a project-local
`node_modules/.bin/wrangler` wins, then a global install, then `npx`. If it can
only be reached through `npx`, `bynk dev` says so — `npx` *downloads* wrangler on
first use, so it's a one-time pause, not a missing tool. Run [`bynk doctor
--only deploy`](/docs/editor-and-tooling/doctor/) to see exactly what you have.

## Deploying

`bynk dev` is for local development only and provisions nothing. Use [Deploy to
Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/)
to provision the required KV namespace and publish a Worker. After that first
deploy, `bynk dev -- --remote` uses the recorded namespace id for remote dev.

If you deployed to a [named environment](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#environments)
(`bynk deploy --env staging`), pass the same name so `--remote` reads the right
one — `bynk dev --env staging -- --remote`. Omit it and `--remote` reads the
default environment, same as before.

## Related

- [Target Cloudflare Workers](/book/guides/projects-build-and-deployment/cloudflare-workers/) — the two emission targets
  and the manual recipe `bynk dev` runs for you.
- [Check your environment with `bynk doctor`](/docs/editor-and-tooling/doctor/) —
  the same capability check `bynk dev` pre-flights.
- Reference: [the `bynk` driver CLI](/docs/bynk-cli/).
