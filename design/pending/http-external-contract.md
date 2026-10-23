---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage
changelog: "Request headers and author-owned error bodies (first-party registry L4, [#2341](https://github.com/bynk-lang/bynk/issues/2341)): an HTTP handler may take `headers: RequestHeaders` (`get`, `bearer`, `clientIp`), and `RawError(status, body, contentType)` answers a 4xx/5xx `ErrorStatus` with a body you choose; tests build headers with `RequestHeaders.of`"
---

## ADR: http-external-contract
title: Services that implement an external HTTP contract
summary: A `headers: RequestHeaders` handler parameter, and `RawError(status, body, contentType)` with compile-time `ErrorStatus` names; amends ADR 0126

**Context.** A Bynk HTTP service could only speak Bynk's own contract. A handler
saw path parameters and a decoded body, never the request's headers: the only
route to a credential was an actor's seam, and an actor describes Bynk's own
schemes (a Bynk-signed bearer token, an HMAC signature, OIDC). The error variants
render Bynk's own body, `{ "error": message }`, or none. The first-party registry
(`design/tracks/first-party-registry.md`, slice R2) implements a protocol that
already exists: the publish API's opaque, operator-issued tokens and its refusal
body, `{ "error": code, "message": text }`, which the `bynk` client and the
conformance suite read. Neither can be written in Bynk as it stood. The accepted
proposal (#2341) has three parts: a `headers` parameter (A), `RawError` (B) and an
`on rejected` handler for the router's own rejections (C). This increment lands A
and B; C follows as its own increment.

**Decision.**

- **A. `headers: RequestHeaders` is a third kind of HTTP handler parameter,**
  beside path parameters and `body`. It may sit anywhere in the list; the router
  passes the request's headers in that position (the dispatch now builds its
  arguments in declaration order, so `body` is no longer assumed last).
  `RequestHeaders` is a built-in type with three methods, each `Option[String]`:
  `get(name)` (case-insensitive), `bearer()` (the token of an
  `Authorization: Bearer …` header) and `clientIp()` (`CF-Connecting-IP`). It is a
  read-only view, read in the handler and never kept: it is refused as a field,
  a non-HTTP parameter, a return type or anything else that crosses a boundary
  (`bynk.types.headers_at_boundary`), and a parameter named `headers` must have
  this type (`bynk.http.headers_wrong_type`). Actors remain the way to authenticate
  a Bynk scheme; this is for a scheme an actor does not describe.
- **B. `RawError(status, body, contentType)` joins `HttpResult` (amends ADR 0126).**
  It answers a 4xx or 5xx status with a `Bytes` body under the given content type,
  and runs no codec, as `Raw` does for a `200`. The status is an `ErrorStatus`
  name: one of the vocabulary's own 4xx/5xx variant names (`Forbidden`,
  `Conflict`, …), bare or as `ErrorStatus.Forbidden`. The checker reads that
  argument syntactically and the emitter writes its number, so a 2xx or 3xx is
  unrepresentable (`bynk.http.raw_error_status`). The existing error variants keep
  their bodies.
- **Tests.** `RequestHeaders.of(["name: value", …])` and `RequestHeaders.empty()`
  build the value a case passes in the parameter's position. At the unit and
  integration tiers it reaches the handler directly; the system-tier driver sends
  the lines as the request's headers, under its own `content-type` and actor
  credential. A case matches a `RawError` by its number,
  `r is RawError(409, _, _)`.

**Deviations from the accepted proposal.**

- **`RequestHeaders`, not `Headers`.** `Headers` is a common user type name (the
  fixture corpus declares one) and the name of a JavaScript global the emitted
  code uses; a built-in of that name would break existing programs.
- **`ErrorStatus` is a closed vocabulary, not a type.** It is recognised only as
  `RawError`'s first argument (`bynk.http.error_status_position` elsewhere), and
  resolved at compile time. Consequences: a status can't be a parameter, so the
  proposal's `fn refusal(status: ErrorStatus, …) -> HttpResult[()]` helper becomes
  a body helper with the status named at the construction site,
  `RawError(Conflict, problem("conflict", "…"), "application/json")`; and a test
  matches the status as an `Int`, not by name. A real `ErrorStatus` type can come
  later without breaking either form.
- **Test headers are an argument, not a `with { … }` clause.** Bynk has no map
  literal and a new clause would be call-site syntax for one parameter; the
  statics need no grammar.
- **A system-tier case reads a `RawError` by its status.** The system tier decodes
  a response by status, as for every variant, so a `RawError(Conflict, …)` reads as
  `Conflict(_)` there; its body is the author's, not `{ error }`.

**Consequences.** A service can authenticate a scheme of its own and answer an
external API's error shape. The wire-contract peek reports each `RawError`'s
status. Without a `headers` parameter or a `RawError`, the emitted output is
unchanged. `on rejected` (C) is still to land, so the router's own `400`, `405`,
`413` and `415` keep Bynk's `{ kind, details }` body for now.
