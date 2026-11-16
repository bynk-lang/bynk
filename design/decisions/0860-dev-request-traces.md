# 0860 — `bynk dev` records per-request traces through a dev-only runtime module

- **Status:** Accepted (v0.627.1)

**Context.** Nothing in the editor or the CLI shows what one request did. The
`bynk dev` event stream has no per-request event, and the emitter already holds
the `.bynk` spans that only the toolchain can attach to a call. The track is
retired (its closing summary is in `design/archive/retired-tracks.md`); its spine is #3580.

**Decision.**

- **Recording sits at the emitter's call sites, with spans passed in.** A traced
  build wraps a cross-context call, an agent call and a capability op in
  `__traceStep(kind, target, span)(call)`. The call is the wrapper's argument, so
  its arguments evaluate where they always did. A step begins, and its clock
  starts, when the wrapped expression starts evaluating, so steps are in the
  order their calls begin; an effectful call is bound with `<-` and so is hoisted
  out of argument position. There are no per-statement hooks. A capability op is wrapped at
  its lowered call site, not through a deps proxy, because only the call site
  knows the span.
- **The trace leaves the Worker as one stdout line.** The generated `fetch` moves
  into `__bynkTracedFetch` and runs under `__traceFetch`, which opens a
  request-scoped record, and prints `__bynk_trace:` followed by one JSON object
  as the response is ready (its fields are in the `bynk dev` reference). The
  driver parses it (slice 1b).
- **The record is found through `AsyncLocalStorage`,** reached through
  `process.getBuiltinModule("node:async_hooks")` because a generated project has
  no `@types/node`. A traced build adds the `nodejs_als` compatibility flag
  (slice 1b). Where the module is unavailable, the request still answers and is
  recorded with no steps.
- **Opt-in, and absent from every other build.** `CompileOptions::trace_requests`
  is off by default and only `bynk dev --trace-events` sets it. The recorder is a
  new runtime module, `trace`, which ADR 0615 emits only when emitted code
  imports it, so no existing runtime module changes and a deploy build is
  byte-identical.
- **A step carries kinds, targets, spans, timings and outcomes only.** No
  argument values. Steps are bounded (200 per request), with a `truncated` flag.
  A step still unsettled when the response is ready reports `pending`.
- **Recording only observes.** A wrapped call returns and throws exactly what it
  did, and outside a traced request the wrapper is the identity.

**Consequences.** A traced build differs from a deploy build in the wrapped
sites and the `fetch` wrapper, so a behavioural fixture runs the same suite with
recording on and off and compares outcomes (slice 1c). The `fault` of a request
is the error's stack, in generated-code coordinates; the driver maps its frames
to `.bynk` spans as it does for `log` events. A synchronous throw from a wrapped
call or its arguments leaves its step `pending` rather than `fault`. A wrapped
site returns a derived promise, so it must be awaited as it is; an RPC stub's
pipelining is not carried over.
