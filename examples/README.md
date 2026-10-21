# Bynk examples

A gallery of small, complete Bynk projects. Each one type-checks, compiles to a
standard Cloudflare Worker, runs locally under `wrangler dev`, and deploys with
`wrangler deploy` — from the same source.

Start with **[`hello-world`](hello-world/)** (a greeter — refined types, a
capability, typed HTTP, a test). The rest each lead with a different part of the
language; the middle four lead with the **agent storage kinds** (`Map`, `Cache`,
`Log`) and the **`Query[T]`** vocabulary.

| Example | Leads with | Entry points | Capabilities | Tests |
|---|---|---|---|---|
| [`hello-world`](hello-world/) | refined types, capabilities, typed HTTP | http | `Logger` | greeting + `Subject` boundary |
| [`greeter-i18n`](greeter-i18n/) | the `Locale` capability, negotiated `Accept-Language`, a message bundle | http | `Locale` | the bundle's `greet` wrapper |
| [`link-shortener`](link-shortener/) | KV persistence with TTL, random ids | http | `Random`, `Kv` | `Slug`/`Url` boundary + key helper |
| [`feature-flags`](feature-flags/) | public vs. authorised routes, KV listing | http | `Kv` | `FlagKey` boundary + key round-trip |
| [`todo`](todo/) | an agent keyed by the caller's identity; a storage `Map` + `Query[T]` | http | — | the agent (add / complete / `pendingCount`) |
| [`orders`](orders/) | storage `Map`s, `@indexed`, and `Query[T]` joins / groups / aggregates | http | — | the joins, groups, and aggregates |
| [`sessions`](sessions/) | a `Cache` with `@ttl` per-entry expiry | http | `Clock` | — (platform clock; see below) |
| [`event-log`](event-log/) | a `Log` with `@retain` + time-window queries | http | `Clock` | — (platform clock; see below) |
| [`rate-limiter`](rate-limiter/) | the agent model (one Durable Object per key) | http | `Clock` | the fixed-window policy |
| [`uptime-monitor`](uptime-monitor/) | scheduled work that calls the outside world | cron + http | `Fetch`, `Kv`, `Logger` | the health policy + key helper |
| [`webhook-relay`](webhook-relay/) | verifying a signed webhook, then forwarding it | http | `Fetch`, `Logger`, `Secrets` | — (all-effectful; see below) |
| [`semver`](semver/) | a real algorithm in a `commons`: version parsing, precedence and range matching | http | — | the library and the service |
| [`markdown`](markdown/) | text processing in a `commons`: Markdown to HTML, escaping first | http | — | inline, blocks and the service |

Together they cover every entry point (http, cron; queue is covered in the
[queue guide](https://bynk-lang.org/book/guides/entry-points/queue/)), both state models — **KV
binding storage** and **Durable-Object agents** — and the full agent storage-kind
catalogue: a `Cell` (`rate-limiter`, `todo`), a `Map` (`todo`, `orders`), a
`Cache` (`sessions`), and a `Log` (`event-log`). The `Query[T]` vocabulary appears
both lazily over storage and eagerly over a `List`: `filter`/`sortBy`/`collect`,
the aggregates `count`/`sum`, the joins `joinOn`/`leftJoin`, and `groupBy`
(`orders`), plus the storage annotations `@indexed`/`@ttl`/`@retain` and the
`Instant`/`Duration` time primitives (`sessions`, `event-log`). They also cover
every actor scheme (`Visitor`, `Bearer`, an authorisation refinement, `Signature`)
and the outbound-`Fetch` + JSON-codec story. `greeter-i18n` covers the `Locale`
capability, real `Accept-Language` negotiation, and a message bundle.

## The shared workflow

From any example directory, one command builds and serves it locally:

```sh
bynk dev          # compile + serve on http://localhost:8787 (local mode)
```

That's the compile-and-run recipe in one step — it runs `wrangler dev` in local
mode, so KV / Durable Objects / queues are simulated and there's nothing to
provision. It compiles in-process; the manual equivalent with `bynkc` is:

```sh
bynkc check .                                     # type-check, no output
bynkc compile . --output out --target workers     # emit a Worker per context
cd out/workers/<name> && npx wrangler dev         # run it locally
```

`bynk` and `bynkc` live at `target/release/` after
`cargo build --release -p bynk -p bynkc` (see the
[install page](https://bynk-lang.org/book/introduction/install/)). The generated
`wrangler.toml` carries the bindings each example needs — a `[[kv_namespaces]]`
stanza, `[[durable_objects.bindings]]`, or `crons` — with placeholder ids to fill
in at deploy time.

To run an example's tests:

```sh
bynkc test .      # from the example directory
```

Ten of the thirteen examples ship tests. Each factors its testable logic — a refined
type's boundary, a key helper, a windowing or health policy, or a
capability-free agent — into a unit that runs without any platform binding (see
*Notes* below for where the line is drawn).

## Notes on the current language surface

These examples are honest about what compiles *today* (Bynk is pre-1.0):

- **Capabilities (`given`) live on handlers, not on free functions.** Effectful
  work stays inside service/agent handlers; only pure helpers are factored out
  (see `uptime-monitor`, `event-log`).
- **Storage `Query[T]` terminals return an `Effect`; the same vocabulary is eager
  over a `List`.** `orders` runs `joinOn`/`groupBy`/`sum` lazily over storage maps
  inside the agent; `event-log` runs the *same* `groupBy` eagerly over a `List` in
  `commons digest`. A test asserts on **scalar** handler results (a `count`/`sum`
  aggregate, a record, a `Result`) rather than on a returned `List`, because a
  `List` terminal in a test needs a typed receiver the test harness does not yet
  track — so `orders` and `todo` test through their aggregate handlers.
- **A test can target a `commons`, an agent, or a context's handlers** — including
  a context that `consumes bynk { … }`. Under `bynkc test` each platform
  capability is a deterministic test double (the clock reads the epoch, `Secrets`
  holds nothing, `Kv` is in memory, `Fetch` never reaches the network), and a
  `stub` overrides any one operation. `sessions` stubs the `Clock` to test expiry,
  `webhook-relay` stubs `Secrets` and `Fetch` to test forwarding, and `event-log`
  drives its time-window routes. See
  [Platform capabilities under test](https://bynk-lang.org/book/reference/testing/#platform-doubles).
