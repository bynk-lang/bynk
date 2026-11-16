---
level: patch
changelog: `bynk/wireContract` carries an HTTP handler's success payload: a `success` ref to the `T` of `HttpResult[T]` and a `payload` per response (#3609)
---

## ADR: wire-contract-success-payload
title: `bynk/wireContract` carries an HTTP handler's success payload
summary: `success` on the HTTP model and `payload` per response, resolved into the existing `types`/`instantiations`

**Context.** For an HTTP handler, `bynk/wireContract` listed the reachable
responses as `{status, variant, origin}` only. The declared return type was
stripped to its `Ok`/200 case, so a client could see which types were reachable
but not which one a successful response body is. The editor's request scenarios
(vscode-bynk#160) need that to check captured and asserted body paths against
the earlier route's response type. `bynk openapi` already resolves `T` per
status, but it compiles from disk, misses unsaved buffers and keys routes
without the unit.

**Decision.**

- **A.** The HTTP model carries `success: WcRef | null`, the `T` of the
  handler's `HttpResult[T]`. It is a ref into the model's existing `types` and
  `instantiations` — the same resolution the envelope's refs use — so a
  generic payload (`Page[Item]`) is a `WcRef::Inst` whose key names an entry in
  `instantiations`. Each `WcHttpResponse` also carries `payload: WcRef | null`:
  `T` for a value variant, `String` for a message variant and for a redirect's
  location, `null` for everything else.
- **B.** `success` is `null` for a non-HTTP handler, a return that is not an
  `HttpResult`, and a `()` payload — which is what a raw-response handler
  (`Raw(..)`) declares — so a client knows there is no typed body to check
  paths against. A raw, streamed or no-payload response has `payload: null`,
  as does a boundary-implicit response (the boundary shapes its body, not the
  handler).
- The payload type is read by one function, `bynk_ide::wire_contract::http_result_inner`,
  which `bynk openapi` now shares, so the two cannot drift. The per-variant
  payload kind comes from the same `http_variant` table.

**Consequences.** Additive: older clients ignore both fields. No language,
checker, emitter or runtime change, and no new type enters `types` — `T` is
already reachable from the handler's return, so only a ref is added. A handler
that declares a non-unit `T` but answers only with raw variants still reports
`success: T`; its `Raw` responses carry `payload: null`, so a client choosing by
status can still tell. Hover, completion, semantic tokens and signature help
are unchanged.
