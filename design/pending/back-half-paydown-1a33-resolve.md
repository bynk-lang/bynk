---
level: patch
changelog: The checker records each call's resolution as closed typed values (a static operation, a store operation, a test address, a first-party capability, a call-site principal) and the emitter reads them instead of comparing names; generated TypeScript and source maps are unchanged. One behaviour tightens: a same-named capability that is not bynk's own `Events` no longer counts as an `Events.emit`
---
refactor(check,emit): the emitter reads resolved callees and store kinds, not names (1A.33)

Closes #3734. Part of #3676.

**What changed.**
- `bynk_check::firstparty` gains `FirstPartyCap` and `resolve`, the single copy of the #934 rule ("is this bynk's own `Idempotency`/`Events`/`Clock`/`Locale`?"). `Callee::Capability` and `CrossCap` carry `first_party`; the checker's own `Events.emit` owner check uses `resolve`.
- `Callee::Intrinsic` is `{ ns: IntrinsicNs, op: StaticOp }`, closed over every static intrinsic (21 namespaces). `Store` and `Query` carry `StoreEntryOp` and `StoreQueryOp` (the store vocabulary `put`/`remove`/`upsert` is disjoint from the value kernels' `MapOp`/`SetOp`, so those were not reusable). `TestService` carries a typed `HandlerAddress`. The ns, op and address strings are deleted.
- `CallSitePrincipal` (`Actor` | `Nobody` | `BadCredential`) is classified once and recorded on the `by` clause (`CallSiteActor` gains an id; `Callee::Principal`). The checker's own matches on the reserved names read it; `bad_signature_driver` is a method on `BadCredential`.
- Emit: the static, codec, capability, store-kind, address and principal decisions read these values; `local_agent_vars` is deleted; a `Connection`-valued map is decided on the model type; `write_header` takes the program so the agent header facts read `StoreFieldIr`.

**Behaviour change, stated plainly.** `is_events_emit`, `block_uses_emit` and the driver's callee filter matched `Events`/`emit` by name only, so a same-named capability from another unit counted as an emit. They now require the capability to be bynk's own (#934's class). No fixture in the corpus has that shape, so no output moves, and a test pins it.

**Goldens and maps.** None change: every positive fixture's `.ts` and `.map` (19,392 files, 3,162 maps) is byte-identical to `main`, apart from the known `1959_system_kv_and_event_delivery` bundle `compose.ts` case order, a HashMap iteration flake (#3815).

**Probes.** No gated probe moves. `wildcard_arms` (trend) does not rise; the new enum matches are exhaustive.
