---
title: Compile and target Cloudflare Workers
---
**Goal:** understand the two emission targets and build a deployable Worker.

`bynkc compile` takes a `--target`:

| Target | Flag | What it emits | Cross-context calls |
|---|---|---|---|
| Bundle | `--target bundle` (default) | A flat TypeScript tree mirroring your source | Direct in-process calls |
| Workers | `--target workers` | One Cloudflare Worker per context | JSON calls over Service Bindings, validated at the boundary |

## Bundle (default)

```sh
bynkc compile . --output out
```

Each source unit becomes a `.ts` file, and contexts call each other directly.
Use this for a single deployable unit or for running the output yourself.

## Workers

```sh
bynkc compile . --output out --target workers
```

Each context becomes a directory under `out/workers/<context>/`:

```text
out/workers/notes/
├── handlers.ts     # your handler logic
├── index.ts        # the Worker entry point + router
├── compose.ts      # dependency wiring
└── wrangler.toml    # Cloudflare config
```

A workers build is the deployable, so it writes no test modules: every `suite`
is stripped (`bynkc test` compiles and stages them itself).

The emitted directory is a standard Worker. Run it locally with
[Wrangler](https://developers.cloudflare.com/workers/wrangler/):

```sh
cd out/workers/notes
npx wrangler dev
```

> **Or, in one step:** [`bynk dev`](/book/guides/projects-build-and-deployment/run-locally/) does this whole recipe for
> you — compile, pick the worker, and `wrangler dev` — from anywhere inside the
> project, with nothing to provision. The manual flow above is what it runs
> under the hood.

> An `from http` service only produces a runnable Worker on the `workers` target.
> A stateful agent compiles to a Durable Object there; on `bundle` the same agent
> uses an in-process state registry instead.

## When a handler faults

A fault in an HTTP route, an `on call` service, an event delivery or a
WebSocket upgrade answers a `500 Internal Server Error`. The client never sees the error. The Worker logs it first, with
`console.error`, so it shows in `wrangler tail` and the Workers logs:

```text
shop.api GET /shout/:word faulted Error: provider exploded for boom
    at Object.shout (index.js:412:11)
    …
```

The line names the context and the dispatch that faulted: the route's
*pattern*, `call <service>` for a service call, `event <service>` for an event
delivery, or `ws <service>` for a WebSocket upgrade. It never names the
request, so no key or value from the URL reaches the log. A queue consumer
logs `queue <name> threw` and retries the message.

Both log the error's stack, not only its message. The fault is caught and
answered, so Cloudflare records the request as `ok` with no exception: the log
line is where it shows.

The frames in that line stay in the bundle's own coordinates
(`index.js:412:11`), even if you deploy with
`wrangler deploy --upload-source-maps`. Cloudflare applies an uploaded source
map only to an exception the Worker does not catch, and a stack the Worker
logs is a plain string it never rewrites. Bynk catches every fault so that it
can answer the `500` or retry the message, so an uploaded map never reaches
these lines. To see a fault at its `.bynk` line, reproduce it under
[`bynk dev --inspect`](/book/guides/projects-build-and-deployment/run-locally/#debugging-the-workers---inspect):
attach a debugger, turn on pausing on caught exceptions, and send the request
again. The debugger stops where the error is thrown, with the call stack on
your `.bynk` lines.

A refused cross-context call is also logged by name on both sides: see
[Contract skew](/book/guides/projects-build-and-deployment/contract-skew/).

## Related

- Tutorial: [Build a small HTTP service](/book/tutorials/02-http-service/).
- [Consume another context's services with `consumes`](/book/guides/program-structure/consume-services/).
- Reference: [emission](/docs/emission/).
