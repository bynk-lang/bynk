# 0818 — A `@raw` handler streams its request body as a `Stream[Bytes]` under the route's cap

- **Status:** Accepted (v0.562.3)

**Context.** A handler's body was always read whole before the handler ran
(`readCapped`, which concatenates every chunk), so the largest upload a service
could accept was bounded by the Worker's memory, even though both ends of a
streamed upload already existed: `RawStreamed` serves a stream (ADR 0665 D),
`R2.putStream` (ADR 0668 B) and `Fetch.sendStream` (ADR 0667 A) consume one.
`reference/http.md` named the gap ("Buffered, not streamed") and #3180 recorded it
as slice 3, after `@form` and `@multipart` (ADR 0792, 0793, 0810). This ADR
records the decisions of the accepted proposal, #3380, and the platform
behaviour that was verified before building, as the proposal required.

**Decision.**

- **A. The type selects streaming, under the existing `@raw`.** A `@raw`
  handler's `body` may be `Stream[Bytes]` as well as `Bytes`; `Bytes` still
  buffers. No new annotation, keyword or `limits` field. The checker exempts
  exactly that parameter from `bynk.types.stream_at_boundary`; a `Stream[Bytes]`
  `body` without `@raw` is `bynk.http.stream_body_not_raw`; every other element
  type stays `bynk.http.raw_body_not_bytes`. A stream is still refused as a
  result, an agent or `call` parameter, a store field and an event payload.
  *Rejected:* a `@stream` annotation (a fourth body kind for a difference that is
  already in the handler's signature).
- **B. The cap is mandatory and enforced as the stream is pulled.**
  `bynk.http.raw_body_unbounded` applies unchanged. A declared `Content-Length`
  over the cap is the `413` it always was, before the handler runs. A body that
  crosses the cap mid-stream throws `BodyTooLarge` from the pull that crossed it;
  the router's fault path answers the same synthesised `413 PayloadTooLarge`
  (stamped, and rendered by `on rejected`) instead of a `500`. A handler's
  response is built only when it returns, so no status has been sent when the
  read fails. No `BoundaryRejection` variant is added. *Rejected:* truncating at
  the cap, and requiring a trusted `Content-Length`.
- **C. The body is a lazy, single-pass, counted `AsyncIterable<Uint8Array>`.**
  `requestBodyStream(request, cap)` (in `http.ts`, beside `claimStream` and
  `byteStreamBody`, not in `boundary.ts` as the proposal sketched: `boundary.ts`
  is also the foreign client's runtime, which has no use for it) reads
  `request.body` by pull, one chunk per pull, counts the bytes actually read, and
  cancels the reader on `return()`, past the cap, or on a source error. It is not
  claimed by the binding: the first consumer claims it (`claimStream`), so a
  second read faults `bynk: a stream can be read only once`.
- **D. Refused with a streamed body:** a `Signature` actor
  (`bynk.http.stream_body_signature`; its HMAC covers the whole buffered body) and
  a multi-actor sum route (`bynk.http.stream_body_on_sum_route`; its wrapper reads
  the body itself). A single-actor `by` check authorises on the head, so it runs
  before any of the body is read.
- **E. Serialisation, retries, idempotency.** A `from http` handler is not
  serialised, and a `Stream` cannot be an agent handler's parameter, so a slow
  upload cannot hold an agent's queue. A failed `R2.putStream` stores nothing, and
  `Fetch.sendStream` consumes its stream whatever the outcome, so a client retry is
  safe. An `Idempotency` key must come from the request head, since the body
  cannot be hashed before it is consumed.
- **F. Deferred.** Streaming a multipart file part (needs its own parser and a
  "a streamed file is the last part" rule) and chunked uploads into R2
  (`Stream.chunks`, or R2's multipart operations). A browser's large upload is a
  `PUT` of the `Blob` to a `@raw` stream route.
- **G. OpenAPI is unchanged:** a streamed `@raw` route documents as a buffered
  one does. **Bundle target:** unchanged in kind; see below.

**Verified before building.** Four platform behaviours were checked on
Miniflare 4 (workerd, local, not the edge), with a plain JavaScript Worker and a
Node client sending a `duplex: "half"` `ReadableStream` body:

1. *Backpressure.* A Worker that read 20 chunks (64 KiB) slowly from a 64 MiB
   body had made the client produce about 4 MB: the read is by pull and the
   sender is held back, but the transport buffers a window of megabytes of its
   own. So "nothing is read ahead" is true of `requestBodyStream`, not of the
   wire; the runtime test asserts the former only.
2. *An early `413`.* A Worker that cancelled the body and answered `413` was seen
   by a fast client as `ECONNRESET` on its write, and by a client that sent
   slowly (5 ms a chunk) as the `413` with its JSON body. So the router answers
   `413`, and whether a client still sending reads it is the client's and the
   edge's; the docs say so.
3. *An unread body.* A handler that returned without reading left the Worker
   healthy (the next request answered normally); a fast client with a large
   unread body saw `ECONNRESET` rather than the response. A handler that answers
   without reading may look like a reset to a client that is still sending.
4. *Responding while reading.* A read after the response has been sent throws
   (`Can't read from request stream after response has been sent`), so a handler
   must finish reading before it returns, which a Bynk handler does by
   construction (its effects complete before it returns). A response body
   *derived* from the request stream, pulling it lazily after the handler has
   returned (an echo of 8 MiB), streams without error, so `RawStreamed` over a
   mapped request body is permitted by the platform. A sequential read of 64 MiB
   then a reply is fine.

*Bundle target.* The HTTP router is `workers_entry.rs` only; the bundle emits an
`on http` handler as a plain method (`http_handler_method_name_ir`) that the host
calls with its parameters. So on the bundle target the host supplies the
`AsyncIterable<Uint8Array>`, there is no body to count, and the cap is the host's
to apply. The in-process test tiers use this form.

**Consequences.**

- *No breaking change.* `body: Stream[Bytes]` on a `@raw` handler was a compile
  error and is now valid; `body: Bytes`, `@form`, `@multipart` and JSON bodies
  are untouched, and `BoundaryRejection` gains no variant (the overrun reuses
  `PayloadTooLarge`, ADR 0586 E).
- The change ships in slices, each with its own tests: the runtime function
  (this one), the checker and emitter, the test tiers and the platform smoke,
  then the docs. The last closes #3380 and #3180.
- A mid-stream `413` can follow side effects the handler has already had (a
  counter bumped before the read); the safe pattern is to read the stream into an
  atomic sink first, as `R2.putStream` is.
- A cap bounds bytes, not time: a client sending a byte a minute holds the
  request open, as with a buffered body. That is the platform's to bound.
