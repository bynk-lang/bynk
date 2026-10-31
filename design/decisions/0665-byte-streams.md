# 0665 — Streams cross capability signatures and ride in records, are read once, and a `RawStreamed` response serves bytes

- **Status:** Accepted (v0.451)

**Context.** ADR 0128 made `Stream[T]` a value type that never crosses a
boundary, and deferred its element semantics (D5) "to the first consumer that
needs `T ≠ Chunk`". ADR 0129 and 0143 gave `HttpResult` a streamed SSE body and a
buffered raw body. R2 (ADR 0585 D-B) and Fetch (ADR 0663 D-D) both buffer whole
bodies and split byte streams out to this slice. Three things stood in the way:

- `bynk.types.stream_at_boundary` fired in a capability operation's signature,
  so no capability could return or take a stream.
- A record field was a boundary position, so a stream couldn't sit beside the
  metadata a consumer returns with it (a status and headers, an object's
  metadata).
- No `HttpResult` variant served bytes as they arrive.

ADR 0023 keeps language and adapter work apart, so this slice is the language
half. The platform sources are their own slices: K5c for Fetch, P5b for R2.

**Decision.**

- **A. A capability operation's signature may carry a `Stream`.**
  - Every provider runs in the caller's isolate: a `provides` in the same
    context, an adapter's TypeScript binding, or the host's `__makeSurface` on
    the bundle target. Nothing is serialised, so the stream passes as the same
    iterable, as a `bynk.crypto` key does.
  - **Elements (settling ADR 0128 D5 here):** any `T`. An element never leaves
    the isolate, so it needn't be serialisable. A stream crossing a context
    boundary stays rejected, and would need its own ADR.
  - **Rejected:** exempting first-party units only. A user's adapter, or a Bynk
    provider standing in for a platform capability in a test, needs the same
    signature.
- **B. A record or sum may carry a `Stream`.**
  - The declaring type, and any type reaching it through a field, payload,
    container or generic argument, is *stream-carrying*. It goes wherever a
    bare stream may, and is refused wherever a bare stream is: a handler's
    parameter or result, a store field, an event, `==`. The error is
    `bynk.types.stream_at_boundary` at the use, with a note naming the field
    chain (`Shelf.latest`, then `Download.body`).
  - It has no JSON codec: `Json.encode` and `decode` refuse it
    (`bynk.types.json_uncodable`), and the emitter writes no `serialise_*` or
    `deserialise_*` helper for it, even in a commons, which otherwise writes one
    for every type it declares.
  - The walk is `bynk_syntax::ast::stream_path`. It follows named and applied
    types into their declarations, a generic record's own fields included, and
    ends on a recursive type.
  - **Rejected:**
    - a built-in opaque type per consumer (`StreamedResponse`,
      `R2ObjectBody`), because each consumer would need its own language
      change;
    - a separate operation for the head and for the body, because the two can't
      be fetched apart without a second request.
- **C. A stream is read once.**
  - Draining a stream, or handing it to a response, consumes it. Reading it
    again faults with `bynk: a stream can be read only once`.
  - **How:** each inline op marks its source under
    `Symbol.for("bynk.stream.read")` and throws if the mark is set. A builder
    marks it when its generator is first pulled, `collect` when it runs, and
    the runtime's response bodies when they're built (`claimStream`). The mark
    is on the iterable, so it holds across modules, and the inline form keeps
    ADR 0128 D6's "no runtime import".
  - **The behaviour change:** a second `collect` of `Stream.of(…)` returned `[]`
    before, and now faults. A platform stream can only be read once, so an
    in-memory one now behaves the same. No fixture or example relied on it.
  - **Rejected:** making `Stream` a held type under ADR 0130's linearity pass,
    which would add mandatory disposal for a rule that needs only one read.
- **D. `RawStreamed(body: Stream[Bytes], contentType: String)`, 200-only.**
  - One `HTTP_VARIANTS` row and a new payload shape. It shares
    `HttpResult[()]` with ordinary variants, as `Raw` and `Streaming` do.
  - **Lowering:** the runtime's lazy pull body, now shared with `Streaming`
    (`pullBody`, `highWaterMark: 0`). Nothing is read until the client reads,
    and an empty chunk is skipped.
  - **No `ETag` and no `content-length`:** the bytes can't be hashed or counted
    without buffering. `@cache` and the other header wrappers still apply.
  - **`HEAD`:** the answer cancels the `GET`'s body rather than dropping it, so
    a streamed body's iterator is returned, releasing a platform stream under
    it.
  - **A fault mid-body errors the body.** The status is already sent, as ADR
    0129 D2 says for `Streaming`.
  - **Rejected:** letting `Raw` take either `Bytes` or `Stream[Bytes]`, which
    breaks the one-variant-one-shape model.
- **E. A system-tier test reads a non-JSON 200 as `Ok`, with its text.** Before,
  a `Raw` 200 at that tier threw on `response.json()`. It now gets the 201
  path's tolerance (ADR 0610), so a `RawStreamed` case can assert `Ok(_)`.

**Consequences.**

- K5c and P5b can return platform streams through their capabilities inside a
  response or object record, and serve them with `RawStreamed`.
- A request body is still read whole (ADR 0586). A streamed `@raw` body is a
  later decision.
- Stream vocabulary stays `of`/`map`/`take`/`collect`. A bounded
  `collectBytes(limit)` is a named follow-on.
