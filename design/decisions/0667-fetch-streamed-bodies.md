# 0667 — Fetch streamed bodies, with a head-only timeout and no replayed redirects

- **Status:** Accepted (v0.453)

**Context.** `receiveBytes` and `sendBytes` (K5b) buffer both bodies. That
caps a proxied download or an upload at a Worker's memory, and holds back the
first byte until the last has arrived. K-streams let a `Stream` cross a
capability's signature, let a record carry one, made streams single-pass
(`claimStream`), and added `RawStreamed`. But no platform source handed out a
stream: every stream in its tests was a `Stream.of` from Bynk.

**Decision.**

- **A. One operation per direction of stream,** beside the byte operations
  and returning `StreamResponse = { status, headers, body: Stream[Bytes] }`:
  - `receiveStream(req: Request, options: FetchOptions)` sends `req` as
    `receiveBytes` does, and returns when the response head arrives;
  - `sendStream(req: Request, body: Stream[Bytes], options: FetchOptions)`
    sends `body` chunk by chunk.

  `StreamResponse` carries a stream, so it can't be a handler's result,
  stored, encoded or compared: that's K-streams' `stream_at_boundary`.
  - **Rejected:**
    - a `Stream` body on `BytesResponse`, which would break every caller;
    - a stream field on `Request`, which would break every literal;
    - leaving uploads to `sendBytes`, since P5b's R2 → Fetch pipe needs them.
- **B. The timeout bounds the response head only.** It's cleared when the head
  arrives, so a body may stream as long as the platform allows. For
  `sendStream` it normally covers sending the body, since the server answers
  after reading it. A failure after the head faults the operation reading the
  stream, since the result is already `Ok`. The byte operations keep
  `AbortSignal.timeout`, which stays armed through the body. The streamed ones
  use an `AbortController` and a timer cleared when the fetch resolves.
  - **Rejected:**
    - bounding the body too, which would need a guessed ceiling for a download
      of unknown size, and would surface as a fault, not `Err(Timeout)`;
    - a second timeout field, which would break every `FetchOptions` literal.
- **C. The response stream is claimed on read.** The binding hands out a fresh
  async generator over the body's reader and doesn't claim it. The first
  `collect`/`map`/`take`, or `RawStreamed`, does, so a second read faults as
  with `Stream.of`. A read that stops early cancels the reader, releasing the
  connection. A `null` body is an empty stream.
- **D. The request stream is claimed on entry,** before any check. Handing a
  stream over consumes it whatever the outcome, as `RawStreamed` does. A throw
  from the caller's stream while it is sent faults `sendStream` with that
  error, rather than reading as `Err(Network)`. Once the response head has
  arrived (a server answering early, say with a 413), the response stands and
  a later throw from the stream isn't reported. A call that doesn't send the
  stream (a refused body or header, a bad timeout, a failed fetch) releases
  it, since its caller no longer can. The body rules are K5b's
  (`InvalidBody` for two bodies, or one on `Get`/`Head`). The upload is
  chunked, with no `content-length`. Node needs `duplex: "half"`, and workerd
  accepts it.
- **E. One stream → `ReadableStream` conversion.** The runtime exports
  `byteStreamBody`, the lazy, empty-chunk-skipping body that `RawStreamed`
  writes with, and the bindings use it for `sendStream`'s body.
- **F. `sendStream` returns every redirect as it is.** A stream can't be
  replayed, so `fetch` would throw on a 307 or 308 under `Follow`, which would
  read as `Err(Network)`. A 301, 302 or 303 would drop the body. So
  `sendStream` always fetches with `redirect: "manual"`.
  - **Rejected:**
    - documenting `Err(Network)`, the misreport `InvalidBody` was added to end;
    - following 301, 302 and 303 in the binding, which would re-implement
      fetch's redirect rules (the hop limit, resolving `location`, stripping
      `authorization` across origins);
    - refusing `Follow` outright.
- **G. Stability.** Both operations are experimental and graduate with
  `sendWith`. As with `BytesResponse`, `StreamResponse` isn't gated itself.
  The browser throws for both.

**Consequences.** On Cloudflare and Node a Worker can proxy an upstream
through `RawStreamed`, or pipe one fetch's body into another, without holding
it.
- The workerd smoke pipes a download that outlasts its 300 ms head timeout
  into a chunked upload. It also re-serves a stream and faults on a second
  read of a platform stream. That's the proof K-streams left to this slice.
- Not covered:
  - an inbound `@raw` request body still arrives whole (ADR 0586), so it can't
    yet be piped straight to `sendStream`;
  - `Service` (interop I5) doesn't stream.
