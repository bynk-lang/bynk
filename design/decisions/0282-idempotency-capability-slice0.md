# 0282 — The `Idempotency` capability, slice 0 — two ops, not one, and why

- **Status:** Accepted (v0.236)

**Context.** The `Idempotency` capability track (spine #921,
[`design/archive/retired-tracks.md`](../archive/retired-tracks.md))
settled §3.1 on a single generic capability op —
`fn dedup[T](key: String, expiresAfter: Duration, compute: () -> Effect[T]) -> Effect[T]`
— that would take the caller's "what to do on a cache miss" as a closure and
handle read-or-compute-and-store internally, correct by construction (no way to
forget the write-back). Slice 0 (#929) implements the capability against the
now-shipped generic-capability-methods mechanism (#926, [ADR
0281](0281-generic-capability-methods.md)) and found this shape does not
compile: `bynk.types.function_at_boundary` explicitly names "capability
operation signature" as one of the positions a function type cannot appear in
(functions cannot serialise or cross that boundary). This is not a gap to work
around — it is the same boundary discipline that already governs record
fields, sum payloads, and service/agent handler signatures — so the
closure-taking shape is dropped, not deferred.

**Decision.**

**A. Two ops, not one.** `capability Idempotency` ships:

```
capability Idempotency {
  fn dedup[T](key: String) -> Effect[Option[T]]
  fn remember[T](key: String, value: T, expiresAfter: Duration) -> Effect[()]
}
```

`dedup` checks for a cached value under `key`; on `None`, the caller computes
its own outcome and calls `remember` to cache it, with `expiresAfter` setting
the retention window from that point. This is the "restrict to a two-call
form" candidate §3.1 originally weighed and set aside in favour of the
closure — now the only mechanically valid option between the two. The
accepted cost: a `dedup` miss with no matching `remember` call simply
recomputes every time, silently — the same trade-off the design notes already
accept for `Sagas.compensate` targeting a non-idempotent operation (§13), not
a new kind of risk this capability introduces.

**B. `remember`, not `record`.** `record` is a reserved keyword
(`bynk-syntax/src/keywords.rs:164`); `Idempotency.record` does not parse.

**C. Every call site names its type argument explicitly, even where an
argument's type would make it inferable.** ADR 0281's Decision B resolves a
generic capability op's type parameter only from an explicit call-site type
argument, never from an argument's type — deliberately narrower than a plain
generic function. `remember[T](key, value: T, expiresAfter)` could in
principle infer `T` from `value`'s type the way an ordinary generic function
call would, but does not: `Idempotency.remember[ReserveOutcome](key, outcome,
24.hours)` is required, not `Idempotency.remember(key, outcome, 24.hours)`.
Confirmed by the compiler: an omitted type argument raises
`bynk.generics.uninferable_type_arg` even though `value`'s type visibly
determines `T`.

**D. Provider given `Clock`; caller does not.** The shipped provider
(`IdempotencyProvider`, identical across the cloudflare/node/browser bindings)
takes `Clock` as its own constructor dependency (`provides Idempotency =
IdempotencyProvider given Clock`, the bodiless/external form ADR 0281's
Decision E requires for a generic op) rather than requiring every `given
Idempotency` handler to also declare `given Clock`. This corrects the design
notes' own worked example (`given Clock, Idempotency`), which predates this
decision; the track doc's §3.1 worked example is updated to match.

**E. Call-site scoping (§3.4) is not implemented in this slice.** The track
doc's settled §3.4 calls for the provider to automatically prefix every
`dedup`/`remember` key with a compiler-synthesised, per-call-site identifier,
closing the cross-call-site collision class of the §6 threat model for free.
This slice ships without it — it is the one piece of §3.4 with no existing
compiler mechanism to copy (a genuinely novel, `Idempotency`-specific emitter
special case), and deferring it keeps this slice's mechanism-proving scope
from also being the first place that special case is designed. The
cross-call-site collision risk therefore remains open, named, not silently
dropped; a follow-up increment closes it.

**Consequences.** `given Idempotency` is a real, checked, usable capability
with one provider (in-memory, ambient, lost on process restart). Verified: a
`tsc_verify`-covered fixture
(`bynkc/tests/fixtures/positive/924_idempotency_dedup_basic`) proves `dedup`/
`remember` compile and pass real `tsc --strict`; the full `bynk-check` and
`bynkc` suites pass unchanged. Not yet done, tracked as open follow-ons: the
call-site scoping mechanism (D above), provider-variant selection and the
durable provider (already deferred to a separate, unfiled future track by
§3.2), and the stub-story fixture for a generic op (already deferred by ADR
0281's own Decision F — `bynk.stub.generic_op` rejects stubbing `dedup`/
`remember`, so tests exercise the real in-memory provider directly, stubbing
only `Clock.now()` for time-dependent cases).
