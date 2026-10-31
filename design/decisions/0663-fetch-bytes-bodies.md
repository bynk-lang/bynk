# 0663 — Fetch byte bodies as one operation per direction, and InvalidBody

- **Status:** Accepted (v0.449)

**Context.** `bynk.Fetch` carried only text. `Request.body` is an
`Option[String]`, and the bindings read every response with `res.text()`,
which replaces bytes that are not UTF-8 with U+FFFD. So a binary download (an
image to copy into R2) or upload (a signed object-store URL) lost data. ADR
0655 found that `Request` and `Response` can't grow a field without breaking
every literal, so the options went into a record beside `send`. While
grounding this slice it turned out that a `Get` or `Head` with `body: Some(…)`
reached `fetch`, which throws a `TypeError`. That surfaced as `Err(Network)`,
which wrongly suggests a retry would help.

**Decision.**

- **A. One operation per direction of bytes,** both returning
  `BytesResponse = { status, headers, body: Bytes }`:
  - `receiveBytes(req: Request, options: FetchOptions)` sends `req` as
    `sendWith` does, its `String` body included, and returns the body as bytes;
  - `sendBytes(req: Request, body: Bytes, options: FetchOptions)` sends `body`
    and returns the response as bytes too. A caller wanting text calls
    `decodeUtf8()`.
  - Both take `FetchOptions`, so there's no options-less twin of each.
  - **Rejected:**
    - one `sendBytes` where a download passes `Bytes.empty()`, because an empty
      body can't be told apart from none, and `Get` can't carry one;
    - a `Body = Text | Binary` sum on a second request type, which would
      duplicate every header rule;
    - a `Bytes` body on `Response`, which would break every caller.
- **B. `FetchError.InvalidBody`,** a variant of the extensible enum. A request
  gets it, with nothing sent, when it has:
  - a `Bytes` body while `req.body` is also `Some`, so neither silently wins,
    as with the typed header slots;
  - any body on a `Get` or `Head`, which the Fetch standard forbids.

  The second rule applies to `send` and `sendWith` too. That changes the
  result of a request that could never succeed from `Network` to
  `InvalidBody`.
- **C. No invented `Content-Type`.** A `Uint8Array` body gets no type from
  `fetch`, and Bynk doesn't default to `application/octet-stream`: the caller
  sets `req.contentType`. An empty `Bytes` is an empty body
  (`content-length: 0`).
- **D. Buffered bodies.** The request body is the `Uint8Array`, and the
  response is `new Uint8Array(await res.arrayBuffer())`. Streaming is a later
  slice (K-streams). The browser playground throws for both operations, as it
  does for `send`.
- **E. Stability.** The two operations are experimental and graduate with
  `sendWith`, whose `FetchOptions` they take. As with `FetchOptions`, the
  type `BytesResponse` isn't gated itself: only the operations that produce
  it are.

**Consequences.** Binary bodies round-trip on Cloudflare and Node. The
workerd smoke sends bytes 0–255 down and back up unchanged. A Worker can copy
an object between an HTTP origin and R2 without a lossy text step. `Service`
(interop I5) shares `FetchProvider.send`, so a `Get` or `Head` with a body
through a Service Binding is `InvalidBody` too.
