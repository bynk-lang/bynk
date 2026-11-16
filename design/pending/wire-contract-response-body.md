---
level: patch
changelog: `bynk/wireContract` describes every HTTP response's body (value, message, redirect, raw, stream) in a new `body` field, checked against `bynk openapi`
---

## ADR: wire-contract-response-body
title: `bynk/wireContract` describes message, redirect, raw and stream response bodies
summary: A `body` descriptor beside the value-only `payload`, classified once and checked against `bynk openapi`

**Context.** [ADR 0870](0870-wire-contract-success-payload.md) gave each
`WcHttpResponse` a `payload`: `T` for a value variant, `null` for everything
else. That left a client unable to tell a message (`{ "error": String }`), a
redirect (no body, a `Location` header), raw bytes and an event stream from a
response with no body at all. A `WcRef` cannot express those: it names a `types`
entry, a base type or an instantiation, and none of them exists there. A bare
`String` for a message was tried and rejected, because a client checking
`$.error` on a 400 would be told the path is invalid (#3632).

**Decision.**

- **A.** `WcHttpResponse` gains `body`, a tagged descriptor, one of
  `{ kind: "Value", ref }`, `{ kind: "Message" }`,
  `{ kind: "Redirect", header: "Location" }`, `{ kind: "Raw" }` or
  `{ kind: "Stream" }`. `Value`'s `ref` is the same ref as `payload`.
- **B.** `payload` is unchanged: the value-only ref. The change is additive;
  a client that reads only `payload` is unaffected.
- **C.** `body` is `null` where there is nothing to describe: a no-payload
  variant (`NoContent`), a `()` value payload, and a boundary-implicit response,
  whose body the boundary shapes and not the handler. `Raw`, `RawTagged`,
  `RawStreamed` and `RawError` are all `Raw` (author-owned bytes, no codec);
  `Streaming` is `Stream` (`text/event-stream`).
- **D.** The classification is one function,
  `bynk_ide::wire_contract::response_body`, over the `http_variant` table.
  `response_payload` is now derived from it. A drift test compares each
  response's body with the description `bynk openapi` gives the same fixture, so
  the two cannot disagree.

**Consequences.** No language, checker, emitter or runtime change, and no new
entry in `types`. `bynk openapi` keeps its own rendering of each body (it also
needs the media type, the `ETag` header and the schema), and the drift test, not
shared code, is what ties the two together.
