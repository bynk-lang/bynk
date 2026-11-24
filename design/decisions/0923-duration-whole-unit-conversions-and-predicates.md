# 0923 — `Duration` converts to whole units towards zero and takes numeric predicates, and `bynk.cloudflare` types its KV spans

- **Status:** Accepted (v0.667)

**Context.** `Duration` had only `toMillis`, so a span in seconds was `d.toMillis() / 1000` (#3771). `Kv.putTtl` took a bare `Int` of seconds, and KV's platform floors (an expiry at least 60 seconds away, a `cacheTtl` of at least 30) were faults at run time because no predicate applied to a `Duration` (#3948).

**Decision.**
- `toSeconds()`, `toMinutes()`, `toHours()` and `toDays()` return `Int` and round towards zero (a span of −90 seconds is `-1` minute). Rejected: a `Result` for a part unit, which is noisy for `max-age`.
- `NonNegative`, `Positive` and `InRange(lo, hi)` apply to a `Duration` base, with `Duration` literal bounds; a numeric bound is `bynk.types.no_numeric_coercion`.
- `AtLeast(lo)` and `AtMost(hi)` are inclusive one-sided bounds on `Int`, `Float`, `Decimal` and `Duration`, with bounds of the base's own type; an `AtLeast` above an `AtMost` is `bynk.types.empty_refinement`.
- A `Duration` literal is admitted to a refined `Duration` at compile time, in a constant, an argument or a field default, as `8080` is to `Port`.
- `bynk.cloudflare` declares `KvTtl = Duration where AtLeast(60.seconds)` and `KvCacheTtl = Duration where AtLeast(30.seconds)`; `Kv.putTtl` (parameter renamed `ttl`) and `KvExpiration.After` take a `KvTtl`, `KvGet.cacheTtl` an `Option[KvCacheTtl]`. `KvExpiration.At` keeps its run-time floor, because how far away an `Instant` is cannot be known statically. The binding takes milliseconds and truncates to whole seconds, so KV's part second rounds towards zero too (one rule); a `KvTtl` is at least 60 whole seconds, so truncation cannot cross the platform floor.

**Consequences.** A too-short literal or constant TTL is a compile error, and a computed one is a `ValidationError` from `KvTtl.of(d)`, a value to handle rather than a fault. Callers of `putTtl` with an `Int` migrate (`examples/link-shortener`, the KV fixtures and the docs in this change). The contract form of the new predicates is `InRangeDur(ms, ms)`, `AtLeast(n)` and `AtMost(n)`, with a `Duration` bound written as its milliseconds; entailment understands the one-sided bounds and the `Duration` range. In OpenAPI a `Duration` is a number of milliseconds, so its bounds publish as `minimum` and `maximum`.
