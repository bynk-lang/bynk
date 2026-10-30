---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: "`bynk`'s `Method` gains `Patch`, `Head` and `Options`; `Fetch.sendWith(req, FetchOptions { timeout, redirect })` adds a timeout (`Err(Timeout)` when it elapses) and a `Manual` redirect policy; `Logger` gains `warn`, `debug` and a structured `log(level, msg, fields)`. `sendWith` and `log` are experimental (standard-library K5a, [#2586](https://github.com/bynk-lang/bynk/issues/2586))"
---

## ADR: fetch-options-and-log-levels
title: Fetch options in a record beside send, more methods, and Logger levels
summary: `Fetch.sendWith` takes a `FetchOptions { timeout, redirect }` record instead of `Request` growing fields, since a new record field breaks every literal; `Method` gains `Patch`, `Head` and `Options`; `Logger` gains `warn`, `debug` and a structured `log` that nests its fields

**Context.** `bynk.Fetch` had four methods, no timeout (so `FetchError.Timeout`
was unreachable), and always followed redirects. `Logger` had `info` and
`error`. ADR 0022 said "widening `Request` later is additive", but a record
literal must give every field (`bynk.resolve.missing_field`), and record fields
have no defaults outside events. ADR 0453 met this when it added `headers`, and
migrated every literal. `Request { … }` is now in 14 `.bynk` files and 3 docs
pages. `Method` and `FetchError` are extensible (track §10 #5), so a `match` on
either already carries a wildcard arm.

**Decision.**

- **A. Options go in a record beside `send`, never in `Request`.**
  - `Fetch.sendWith(req: Request, options: FetchOptions) -> Effect[Result[Response, FetchError]]`,
    with `FetchOptions = { timeout: Option[Duration], redirect: Redirect }` and
    `Redirect = enum { Follow, Manual }`.
  - This corrects ADR 0022: widening `Request` is breaking. A future request
    option is a field of `FetchOptions`, which only `sendWith` callers build,
    and the same choice (an options record beside the plain operation, ADR
    0051) as `R2.putWith` and `Queue.sendWith`.
  - **Rejected:** fields on `Request` (a second migration of every literal),
    and an operation per option (`withTimeout`, `withRedirect`).
- **B. A timeout bounds the whole exchange.** `AbortSignal.timeout(ms)` is the
  request's signal, so it covers the response body as well. When it elapses
  the result is `Err(Timeout)`. A timeout of zero or less faults the call
  before anything is sent, as `Random.bytes(0)` does. `send` stays exactly
  `sendWith` with no timeout and `Follow`.
- **C. Two redirect policies.** `Follow`, and `Manual`, which returns the `3xx`
  with its `location`. `Redirect` is extensible, so an `Error` policy can be
  added once checked against current Workers behaviour.
- **D. `Method` gains `Patch`, `Head` and `Options`.** The bindings already send
  `tag.toUpperCase()`, which matters for `PATCH`: the Fetch standard normalises
  the case of only the other six. A `Head` response's `body` is `""`.
- **E. Logger levels and structured fields.**
  - `warn` and `debug` write the message through the console method of that
    name.
  - `log(level: LogLevel, msg: String, fields: Map[String, String])` writes
    `{ message, level, fields }`, with `level` in lowercase: one object on
    Cloudflare (Workers Logs indexes its keys), one JSON line under Node, and
    the message then the fields in the browser.
  - **Fields are nested**, so a field named `message` or `level` cannot
    overwrite the record's own, and nothing is dropped.
  - **Values are text.** A `Map[String, Json]` would need a JSON value type the
    kernel does not have.
  - **Rejected:** a `Logger.with(fields)` returning a scoped logger, since a
    capability operation cannot return a capability.
- **F. Stability.** The new `Method` variants, `warn` and `debug` are stable, as
  their enum and capability are. `sendWith` and `log` are experimental
  (`bynk.Fetch.sendWith`, `bynk.Logger.log`), the first operation-level
  experimental marks in the stable `bynk` unit. Their types (`FetchOptions`,
  `Redirect`, `LogLevel`) are not gated, as `QueueSend` is not.
- **G. `Bytes` bodies are a separate slice (K5b).** They need request and
  response shapes of their own, for the same reason as A.

**Consequences.** Existing code compiles unchanged. A service can bound an
outbound call, see a redirect before following it, and log queryable fields.
`FetchError.Timeout` is now reachable. The test doubles gain every new
operation (`sendWith` faults until stubbed, as `send` does), and a workerd smoke
pins the timeout and both redirect policies against the platform's own `fetch`.
