---
level: minor
changelog: "`bynk.cloudflare`'s `R2` gains streamed reads and writes: `getStream` gives an `R2Body` (the object's metadata and its body as a single-pass `Stream[Bytes]`, served with `RawStreamed`), and `putStream`/`putStreamWith` write a stream of a declared `size`, a mismatch faulting with nothing stored. Multipart uploads (`createUpload`, `uploadPart`, `completeUpload`, `abortUpload`) build one object from whole-`Bytes` parts across requests, through a plain `R2Upload` handle (standard-library P5b, [#2626](https://github.com/bynk-lang/bynk/issues/2626))"
---

## ADR: r2-streams-multipart
title: R2 streams objects through a declared size and uploads them in parts through a plain handle
summary: `R2.getStream` gives metadata plus an unclaimed single-pass `Stream[Bytes]` wrapping R2's body; `putStream`/`putStreamWith` take a declared `size`, enforced by the binding and passed through `FixedLengthStream` on Workers; multipart uploads take whole-`Bytes` parts and a codable `R2Upload { key, uploadId }` rebuilt with `resumeMultipartUpload` on every call; all inherit `R2`'s experimental tier

**Context.** ADR 0585 (P5) shipped `R2` over whole `Bytes` objects. Its
decision B split byte streams out because they needed language work. That work
landed as K-streams (#2623): a `Stream` may now cross a capability signature
and ride in a record, every stream is single-pass, and `RawStreamed` serves a
`Stream[Bytes]` as it is pulled. P5b is the platform half for R2. ADR 0023
keeps it apart from the language half, so this slice changes no checker,
parser or lowering.

Three platform facts shaped it:

- **R2's body is a `ReadableStream`.** A Bynk `Stream[Bytes]` is an
  `AsyncIterable<Uint8Array>`.
- **A streamed put needs a known length.** Cloudflare's documentation doesn't
  say so. workerd refuses a stream of unknown length with "Provided readable
  stream must have a known length (request/response body or readable half of
  FixedLengthStream)", observed on Miniflare with this binding's
  `FixedLengthStream` disabled.
- **Miniflare enforces multipart's limits.** It reads `complete`'s part list in
  the order given. It refuses a non-last part under 5 MiB with "Your proposed
  upload is smaller than the minimum allowed object size. (10011)". Both were
  observed in the workerd smoke.

**Decision.**

- **A. `getStream(key) -> Effect[Option[R2Body]]`.**
  - **The record.** `R2Body = { object: R2Object, body: Stream[Bytes] }`, which
    nests P5's metadata record. `R2Object` keeps its codec. `R2Body` is
    stream-carrying, so it is non-boundary.
  - **The body.** The binding wraps R2's `ReadableStream` in an async generator
    over its reader. It's an `AsyncIterable` on every platform and under
    `tsc --strict`. Its `return()` cancels the reader, so a cancelled response
    or a `take` releases R2's stream.
  - **Claiming.** The binding doesn't claim the stream it hands out: its reader
    does (`collect`, `map`, or `RawStreamed`'s body), so a second read faults.
    Claiming it in the binding would make the first `RawStreamed` throw.
  - `get` is unchanged: `Bytes` stays the right type for a small object.
- **B. `putStream(key, body, size)` and `putStreamWith(key, body, size,
  R2Put)`.**
  - The binding claims the stream it consumes and counts its bytes.
  - Where the platform has `FixedLengthStream` (Workers, Miniflare), the bytes
    go through one of `size`. Elsewhere (Node: the binding tests and the
    integration tier), the counted stream goes as it is.
  - **Mismatches.** Too many bytes, too few, or a size that isn't a whole
    number faults with one message on every tier:
    `bynk.cloudflare.R2.putStream: the stream held N bytes, not the declared
    size M`. R2 stores nothing.
  - **Source failures.** A source stream that throws fails the write with its
    own error.
  - `putStreamWith` mirrors `putWith`: `None` when its condition fails.
- **C. A streamed write consumes its stream inside the effect.** Its value is
  the stored `R2Object`.
- **D. Multipart uploads.**
  - **The handle is plain data.** `createUpload(key, R2UploadOptions)` returns
    `R2Upload { key, uploadId }`, which is codable. An upload can span
    requests: the client keeps the handle, or an agent does.
  - **Every operation rebuilds the upload** with
    `resumeMultipartUpload(key, uploadId)`, so no live object outlives a call.
    The operations are `uploadPart(upload, n, Bytes) -> R2Part { partNumber,
    etag }`, `completeUpload(upload, parts) -> R2Object` and
    `abortUpload(upload)`.
  - **Parts go in number order.** `completeUpload` sorts them before handing
    them to R2, since R2 reads them in the order given.
  - **R2's refusals are faults.** That covers a bad part list, an unknown or
    expired upload, and undersized parts.
  - **Parts are whole `Bytes`,** because a part fits in memory by design.
- **E. The doubles.**
  - The unit double (`R2_DOUBLE_BODY`) and the integration double
    (`makeIntegrationR2`) stream bodies in 64 KiB chunks.
  - They enforce the declared size with the binding's message.
  - On completion they check part numbers from 1 to 10,000, each listed part's
    etag, and uniform sizes but for a smaller last part.
  - They don't enforce the 5 MiB minimum, so a test needn't build 5 MiB of
    `Bytes`. The workerd smoke pins that R2 does.
- **F. Stability.** `"R2"` is listed whole in `Unit.experimental`, so every
  new operation is experimental with no new entry.

**Consequences.**

- **Constant-memory serving.** A handler serves an object of any size with
  `RawStreamed(o.body, …)` in constant memory. It copies an object stream to
  stream with `putStream(dst, o.body, o.object.size)`. An object larger than
  one request arrives in parts.
- **A client's upload still can't stream into R2.** A `@raw` request body is
  read whole (ADR 0586), so multipart is the route for large uploads until a
  streamed request body exists.
- **Not done yet:** ranged reads, conditional reads, streamed parts, and typed
  multipart errors. They are left to a consumer that needs them.
- **Verified locally, not on real R2.** The binding's behaviour is pinned on
  Miniflare (workerd) by `binding_workerd.rs`: the streamed round trip, the
  size faults, a cross-request multipart upload of an 8 MiB part, and the
  minimum-size refusal. It hasn't been run against production R2.
