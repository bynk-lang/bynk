# 0610 — A raw-body 201 for HTTP handlers

- **Status:** Accepted (v0.403)

**Context.** A handler could answer author-owned bytes only at `200` (`Raw`,
`RawTagged`, ADR 0143 and the caching-raw-responses ADR) or at an error status
(`RawError`, #2341). The one variant that carries `201`, `Created(value)`, always
runs the codec, and the codec writes an `Option` field in its tagged form
(`{"kind":"None"}`). The first-party registry (`design/tracks/first-party-registry.md`,
slice L6) must answer a new version's publish `PUT` with `201` and the signed
index line in the protocol's own shape. Its `PUT` answer goes through `Created(l)`,
so the body carries `"toolchain":{"kind":"None"}`. A strict index-line parser in
the client's conformance suite (#2401) refuses that body, and nothing the service
could write fixed it, because the status comes from the variant.

**Decision.**

- **A. A `RawCreated(body: Bytes, contentType: String)` variant.** It adds one
  `HTTP_VARIANTS` row at `201` with the existing `HttpVariantPayload::Raw` shape.
  So `Raw`'s construction check (`bynk.types.variant_arity`,
  `bynk.types.argument_mismatch`), its `variants_of` binding (`body`,
  `contentType`) and the emitter's payload-field names cover it unchanged, and
  completion lists it from the registry. The runtime gains the union member, its
  constructor, and a `201` writer that is `Raw`'s without the validator. Every
  existing `Raw(…)` is unchanged. Rejected: a success status on raw responses
  (`RawStatus(Created, body, contentType)` with a `SuccessStatus` name set like
  `ErrorStatus`). It is more general, but it adds a second status-name namespace,
  and a 3xx needs `Location` semantics a raw body does not give. A `RawAccepted`
  is added when a contract needs one.
- **B. No validator.** A `201` answers a write and is not cached, so `RawCreated`
  carries no `ETag` even where the `GET` dispatch passes `weakEtag`. The router's
  `304` step therefore never fires for it.
- **C. The handler type is unchanged.** As for `Raw`, the payload ignores `T`, so
  `RawCreated` mixes with `Ok(value)`, `RawError(…)` and the rest in one
  `HttpResult[T]` handler.
- **D. Tests read it by tier.** At the unit tier a case matches
  `RawCreated(_, "application/json")` or binds its fields. The system tier
  decodes a response by its status, as it does for `RawError`, so a `RawCreated`
  reads as `Created(_)`. The `201` decode now tolerates a body that is not JSON
  and takes its text as the value; a JSON body decodes as before.

**Consequences.** A Bynk service can implement an external contract's `201`
answer byte for byte. The registry's switch of its `PUT` answer to `RawCreated`
is a follow-up in the service's area: it needs #2400's `renderLine`. Hover and
signature help carry no per-variant entry for any built-in `HttpResult` variant,
`Raw` included, so they are unchanged. Semantic tokens are unchanged too. `@cache`
is legal only on a `GET`, and `applyCache` stamps whatever response the route
built, so a `GET` that answers `RawCreated` would carry the route's
`Cache-Control`, as `Created` already does. The checks are a positive fixture
(`2403_http_raw_created`, whose unit and system suites run), the arity negative,
and runtime unit tests of the writer and the `201` decode.
