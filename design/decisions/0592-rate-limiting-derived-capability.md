# 0592 — Rate limiting as a derived capability

- **Status:** Accepted (v0.394)

**Context.** The only way to rate-limit a Bynk service was an agent: one Durable
Object per key, exact, at the cost of a DO call per request, which is the wrong
tool for coarse abuse limiting at the edge. Cloudflare's Rate Limiting binding
is that tool: `env.X.limit({ key }) → { success }`, configured by a
`[[ratelimits]]` stanza (`name`, `namespace_id`, `simple = { limit, period }`,
`period` 10 or 60), counting per location and eventually consistently. Its
`namespace_id` is a positive integer the user chooses, unique per account
(track §11 Q7). The first-party registry's R5 (#2308) needs per-token limits.
This follows the derived-capability pattern of ADR 0469 (S4) and R2's P5.

**Decision.**

- **A. `RateLimit` is an instance-only derived capability with required `@limit`
  and `@period`.** An `Instantiable` row (prefix `RATELIMIT`, `bare: false`):
  `capability PublishLimit = RateLimit @limit(20) @period(60.seconds)` binds as
  `RATELIMIT_PUBLISH_LIMIT`. `@limit` is a positive `Int` literal; `@period` a
  `Duration` literal of exactly 10 or 60 seconds (compared by value, so
  `1.minutes` is accepted). A bare use is `bynk.capability.instance_only`, a
  missing option `bynk.capability.missing_option`, and any other value
  (zero, a negative — which parses as a unary minus, not a literal — a string,
  another window) `bynk.capability.invalid_option`. The notes of all three now
  show the kind's own example (`capability LoginLimit = RateLimit @limit(100)
  @period(60.seconds)`) instead of the `Service` one. Rejected: a bare
  `RateLimit` taking the limit per call (the platform fixes it in
  configuration); an options record (breaks one-annotation-per-option).
- **B. One operation, `allow(key: String) -> Effect[Bool]`.** It calls
  `limit({ key })` and returns `success`. Named `allow`, not `limit`, because
  `@limit` already sets the count. A `Bool`, because the result has one field;
  being over the limit is an ordinary outcome, not a failure. The docs state
  that counting is approximate, per location, and that exact quotas are an
  agent's job.
- **C. P3 is Rate Limiting alone.** Analytics Engine moves to its own slice (P3b
  in the track's §9), as P5 split streams out. Per-IP limiting needs a handler
  to read `CF-Connecting-IP`, a request-surface follow-on; per-token,
  per-account and per-route limits need nothing more.
- **D. `bynk deploy` allocates `namespace_id` in the ledger.** A new
  `ratelimit_instances` map (environment → worker → binding →
  `{ namespace_id }`), additive and omitted when empty. A missing id starts at
  a seed — 1000 plus a hash of (environment, worker, binding) into a span of a
  billion — and probes upward past the ids any environment of this ledger
  holds. It is minted locally (no account call) and written to the lock before
  the push (ADR 0180). The seed is what keeps two Bynk projects on one account
  apart (review of #2345): each has a ledger of its own that can't see the
  other's ids, but a Worker's name is unique on an account, so their limiters
  start in different places and meet only by a hash collision; a fixed start
  (the first draft's "smallest from 1000") gave every project's first limiter
  the same id. CI refuses an unrecorded limiter, as it does a KV namespace. The
  emitted `namespace_id` is a placeholder (`<RATELIMIT_PUBLISH_LIMIT_NAMESPACE>`)
  that the push materialises structurally; the `[env.<name>]` block copies the
  materialised top-level stanzas. `bynk dev` leaves the placeholder, which
  Miniflare accepts (verified: it simulates the binding and enforces the
  limit); `--remote` materialises from the ledger, refusing an unallocated
  limiter. Changing `@limit`/`@period` is a redeploy that keeps the id; a
  removed limiter's row is forgotten once the Worker's next push succeeds (it
  holds no data and no account resource, but until then the live Worker still
  binds its id). Ids a Worker outside Bynk chose can't be detected without a
  listing API, which the docs state. Rejected: a bare hash with no ledger
  (two of this project's limiters could collide, silently, with nothing to
  probe past); an id in source (A12).
- **E. The widenings are general.** `Instantiable.options` is now
  `&[(&str, OptionKind)]` (`Str`, `PositiveInt`, `Duration`), and
  `instance_option_value` dispatches on the option's kind;
  `DerivedInstance.options` holds an `OptionValue` (`Str`, `Int`, `Seconds`).
  The string options (`worker`, `jurisdiction`) are unchanged. The TOML writer
  gains an inline-table value. `context_bindings` now matches every
  instantiable kind by name, with no catch-all, so a new kind can't silently
  bind as a KV namespace.
- **F. Tests at every tier; experimental.** Each instance's double counts per
  key over windows of its `@period`, on the harness clock, so a case can
  exhaust a limiter and recover it with `advance Clock by`: at `unit` through
  the double (its factory takes the limit and period), and at `integration`
  and `system` through `makeIntegrationRateLimit`, bound in each participant
  Worker's env and given the harness clock (it stands still at 0 without one,
  so answers never depend on wall time). The binding has R-13 tests and a
  `workerd` smoke. `RateLimit` is experimental behind `[unstable] allow =
  ["bynk.cloudflare.RateLimit"]` until the registry has run it.

**Consequences.** A service declares a limiter in one line and checks it in one
call. The approximate count can let a key through somewhat over its limit; the
docs point exact quotas at an agent. A `namespace_id` collision with a
non-Bynk Worker on the same account would share counters, undetectably. The
instance-option machinery now takes typed literals, for the next kind that
needs them. Completion offers the instantiable kinds after `capability X =`, a kind's
required options as a snippet after it, `10.seconds`/`60.seconds` inside `@period(`, no
longer offers the handler `@limit`'s `maxBody` inside an instance's `@limit(`,
and a limiter's hover shows its emitted `simple = { … }`. No existing golden,
ledger or deploy output changes.
