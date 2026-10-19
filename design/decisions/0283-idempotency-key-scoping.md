# 0283 — Idempotency key scoping — the qualified handler path, not a hash

- **Status:** Accepted (v0.237)

**Context.** [ADR 0282](0282-idempotency-capability-slice0.md) (§3.4, D)
shipped slice 0 of the `Idempotency` capability without any automatic scoping
of the developer-supplied `key` — two unrelated handlers that happened to pass
the same literal key would silently share one cache entry. This was named as
an explicit follow-up, not a slice of
[`design/archive/retired-tracks.md`](../archive/retired-tracks.md)
itself (it's a correctness fix to the shipped mechanism, not new capability
surface).

Two candidates were weighed: hash the call site's source span into the key
(opaque, but the emitter infrastructure that tracks spans for source maps is
already present at every lowering site), or prefix the key with the calling
handler's own qualified name (`<context>.<service or agent>.<handler>`, e.g.
`shop.reserve.ordering.call`) — readable, deterministic, no hashing, but
requires threading a new value through each place a capability call can
lower.

Auditing every site where `cx.capabilities` is populated (the condition under
which a capability call can lower at all) found the real surface much smaller
than "thread a string through every `LowerCtx` construction": a plain method
and a free function never populate `capabilities` (no `given` clause reaches
them) and are unreachable for this; an agent's `Cell` initialiser and its
invariant/transition predicates likewise never populate it. The genuine sites
are four: an ordinary service handler (`emit_service`), an agent handler
(`emit_agent`), a composed provider's own op body (`emit_provider` — a
provider can depend on and call `Idempotency` itself), and a websocket
lifecycle DO method (`emit_ws_do_method`). A source-span hash would still need
new plumbing at exactly these same four sites, so the two candidates don't
actually differ on implementation cost — the handler-path prefix was chosen
purely for the readable-key property it gives for free (you can see which
handler wrote an entry by reading the stored key), not because it was
cheaper to wire up.

**Decision.**

**A. The prefix is `<unit qualified name>.<service or agent name>.<handler
name>`,** joined with the key by `::` inside a template literal — e.g.
`` `shop.reserve.ordering.call::${orderId}` ``. The unit name comes from
`EmitProjectCtx::commons_name` (already computed for every unit kind, not
only `context`, so a provider inside an `adapter` scopes correctly too — this
path is wired the same as the other three but currently unexercised by any
fixture, since every provider `bynk.bynk` itself declares, including
`Idempotency`'s own, is external/bodiless and so never reaches `emit_provider`'s
body-lowering branch at all; it matters for a *third-party* adapter's
Bynk-bodied provider that itself depends on `given bynk.Idempotency`). Only
the key argument (always the operations' first parameter) is rewritten;
`value` and `expiresAfter` are untouched.

**B. Scoping is threaded via a new `LowerCtx::handler_scope: Option<String>`
field,** set once per real call site (the four above) rather than derived
per-call from context already on `LowerCtx` — the existing fields
(`capabilities`, `in_agent_handler`, `ws_self_agent`, …) don't uniquely
identify *which* handler is running, only what's callable from it.

**C. A missing scope at the point an `Idempotency.dedup`/`remember` call
actually lowers is a compiler panic, not a diagnostic or a silent no-op.**
`handler_scope` is `None` by default; if the emitter grows a fifth site that
populates `capabilities` without also setting `handler_scope`, an
`Idempotency` call reaching it fails loudly (a debug-visible panic during
that build) rather than silently shipping an unscoped key — the failure mode
this change exists to close in the first place. No fixture can exercise a
missing scope directly (every real site sets one), so a `bynk-emit` unit test
(`emitter::lower::idempotency_scoping_tests::missing_handler_scope_panics`)
constructs a `LowerCtx` directly and asserts the panic, alongside two
companions proving the present-scope and non-first-party paths behave as
claimed. Both of the two lowering paths a capability call can take — the
direct in-scope form
(`Idempotency.op(...)`) and the cross-context qualified form
(`B.Idempotency.op(...)` / `Alias.Idempotency.op(...)`) — go through the same
scoping check.

**D. This changes the emitted cache key for every existing `Idempotency`
call site** (confirmed against the slice-0 fixture, whose golden key changed
from `orderId` to `` `shop.reserve.ordering.call::${orderId}` ``). Nothing
deployed against slice 0 depends on the old key format — the capability
shipped without any durability guarantee across restarts (single in-memory
provider, ADR 0282 §Consequences), so there is no persisted state whose key
format this could silently orphan.

**E. Scoping keys off the capability's *identity*, not its name.** Rebuilding
the slice-0 fixture's goldens surfaced a real bug in an early version of this
change: matching on the bare capability name `"Idempotency"` alone also
scoped three unrelated, already-shipped fixtures
(`921_generic_capability_op_adapter`, `922_…_flattened_rebrand`,
`923_…_qualified_cross_context`) that declare their *own*,
unrelated `capability Idempotency` purely as ADR 0281's illustrative example —
a same-named capability from a different adapter is not first-party `bynk`'s
`Idempotency` and must not be scoped. The fix distinguishes them by
provenance, not name: the cross-context-qualified path already resolves the
consumed unit's name (`CrossContextInfo::resolve_cross_capability`) and checks
it's literally `"bynk"`; the flattened path checks
`CrossContextInfo::flattened_caps` (which unit a bare name was flattened in
from) and a new `LowerCtx::in_bynk_unit` flag (true only when the emitted unit
*is* `bynk` itself, for a capability call from one first-party provider body
into another) — `bynk` being a reserved namespace makes both checks exact.
Confirmed: re-blessing after the fix left the three unrelated fixtures'
goldens untouched and only changed the two genuine `Idempotency` fixtures.

**Consequences.** A dedup/remember key collision across unrelated handlers is
now impossible without deliberate effort (calling `Idempotency` cross-context
under an alias that happens to resolve to the same handler is the only way
two calls can legitimately share a scope, and in that case they *are* the
same call site). The named "Slice 1" follow-up (event-subscriber sugar for
`e.eventId` as the canonical dedup key) and the durable/platform-provider
direction (§3.2/§3.3) remain unaffected and unfiled.
