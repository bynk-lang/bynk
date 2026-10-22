# 0586 — Raw HTTP request bodies

- **Status:** Accepted (v0.391)

**Context.** A `body` parameter was always parsed from JSON before the handler ran
(`await request.json()`, then the deserialiser). `body: Bytes` was accepted, but its
wire form is a base64 JSON *string*, so a client had to base64-encode and wrap the
payload (a third larger), and a non-Bynk client (`curl --data-binary`, the `bynk`
publish client's `application/vnd.bynk.tree-v1` upload) could not call it at all.
The request-body limit of ADR 0165 was a `Content-Length` fast-reject only, so a
chunked upload, or one with a false length, was buffered whole. The first-party
registry (`design/tracks/first-party-registry.md`, slice L1) needs a `PUT` of an
archive written in Bynk. ADR 0143 gave the *response* a raw body (`Raw`); this is
its request-side counterpart.

**Decision.**

- **A. `@raw(contentType: "…")` marks a handler's `body: Bytes` as the raw request
  body.** A third handler annotation, beside `@cache` and `@limit` (ADR 0163, 0165),
  legal on `POST`/`PUT`/`PATCH` only. `contentType` is required: a `type/subtype`, or
  `"*/*"` to accept any; parameters such as `charset` are not part of the match. The
  `body` must be `Bytes`. Checks: `bynk.http.raw_on_bodyless`, `raw_duplicate`,
  `raw_unknown_arg`, `raw_bad_content_type`, `raw_body_not_bytes`. A `body: Bytes`
  without `@raw` keeps its base64-in-JSON form, so existing callers are unaffected.
  Rejected: making `body: Bytes` always raw (silently breaks every base64 caller by
  changing nothing visible); a new `RawBody`/`Upload` type (a type for what is a
  boundary policy, which already lives in annotations).
- **B. The router checks `Content-Type` and answers `415`.** The request's media
  type, parameters stripped and compared case-insensitively, must equal the declared
  one unless it is `"*/*"`; a missing or different one is a synthesised
  `415 UnsupportedMediaType` (`{ kind: "UnsupportedMediaType", details }`), stamped
  with the CORS and security headers. It sits where ADR 0165's `413` sits: after the
  method check, before the auth seam and the body read. It is a router decision, not
  a value a handler returns, so `HttpResult` is unchanged.
- **C. A raw route must have a cap, and every capped body is counted as it is
  read.** A `@raw` handler needs an effective `maxBody` (`@limit` or the service's
  `limits`), else `bynk.http.raw_body_unbounded`; an unbounded binary upload is the
  denial of service ADR 0165 exists to stop. Capped bodies are read through the
  runtime's counting reader (`readCapped`, already used for `/_bynk/call`, now
  exported with a cap parameter): a chunked body, or one whose `Content-Length`
  understates it, is cut off at the cap with `413`, and the `Content-Length`
  fast-reject stays as the cheap first check. This **amends ADR 0165's
  fast-reject-only posture**, for JSON routes too (one counted read before
  `JSON.parse`). On a multi-actor sum route the wrapper owns the body read, so the
  counted read is there, and an over-cap body is a returned `PayloadTooLarge`.
- **D. A `Signature` actor verifies the raw bytes.** On a `@raw` route the HMAC is
  computed over the octets as received (`verifySignatureHmacSha256` takes a
  `string | Uint8Array`; a timestamp prefix is the same bytes either way) and the
  same bytes go to the handler. No new actor surface.
- **E. Tests.** Unit and integration tiers are unchanged (the value is passed to the
  handler). The system tier's driver sends a typed call's `Bytes` as they are, under
  the declared type (`application/octet-stream` for `"*/*"`), and signs the octets.
  `BoundaryRejection` gains `PayloadTooLarge(detail)` and
  `UnsupportedMediaType(detail)`, classified by the `kind` of the router's body (a
  handler's own `PayloadTooLarge` has none, so it stays `Handled`). This is a
  breaking change to a closed sum: a test that matches `BoundaryRejection`
  exhaustively without a wildcard needs two new arms (the in-repo corpus has none).
  A `Wire` body is text, so on a `@raw` route the driver sends it as `text/plain`: a
  concrete-type route answers `Rejected(UnsupportedMediaType(_))`, and a `"*/*"` route
  takes it, so `Wire` probes both the `415` and the cap.
- **F. Docs and tooling.** `http.md` gains "Raw request bodies", the counted read
  replaces the fast-reject caveat in "Request body limits", and the lifecycle shows
  the `415`. `@raw` is offered in completion (annotation snippet, `contentType`
  argument) and hovers with its content type and effective cap. The wire-contract
  peek gains a raw request-body form.

**Consequences.** The registry's publish API (`PUT /api/:org/:package/:version`
under `application/vnd.bynk.tree-v1`) and any upload endpoint can be written in
Bynk, receiving the archive exactly as sent. The body is buffered up to the cap;
streaming a request body into the handler, multipart or form decoding, and reading
request headers in a handler are not part of this. The content-type check is a
contract, not a security boundary; the handler still validates what it receives.
The counted read touches every capped JSON body route's emitted code (mechanical).
The wire-contract peek (ADRs 0305-0308) shows a `@raw` route's request as the
raw octets of `body` under the declared media type, and lists the `413` and `415`
among the responses the boundary injects. The `limits` section's field syntax is
`maxBody: N`, not the issue's `maxBody = N`.
