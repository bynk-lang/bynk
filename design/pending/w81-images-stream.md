---
level: minor
changelog: `Images.transformStream` reads an image as a `Stream[Bytes]` and answers an `ImageStream` that `RawStreamed` serves as it is read (experimental; #3515)
probes: emit_diagnostics, emit_abi_shapes, ts_writes
---

## ADR: images-transform-stream
title: `Images.transformStream` is `transform` over streams, and its result is not data
summary: A streamed form of `Images.transform`: `Stream[Bytes]` in, `ImageStream` out, refusals before the stream starts are values

**Context.** `Images.transform` buffers the whole image in and the whole result out. For a large image a handler wants to serve the result as it is produced, as `R2.getStream` does for an object. The proposal (#3515) put the streamed form in a second slice.

**Decision.** `Images` gains `transformStream(image: Stream[Bytes], options, output) -> Effect[Result[ImageStream, ImagesError]]`, where `ImageStream` is `{ body: Stream[Bytes], contentType: String }`: read once (`collect`, `map`, or `RawStreamed`), and not data (it cannot be stored, sent or encoded), like `R2Body`. The options and output are `transform`'s; the overlay images stay `Bytes`, since an overlay is small and a stream cannot be read twice for `repeat`. The input stream is claimed when the call starts and read as the platform pulls it; reading a stream twice is a fault, not an error value (also when the call is then refused), and a source refused before it was read (an option out of range) has its iterator closed, which is a no-op for a source that has not started, as for `R2.putStream`. A producer that throws while the platform pulls it stays a fault, not an `ImagesFailed`. A refusal before the stream starts (the platform's `ImagesError` on `output()`, an option out of range) is the `Err`, so a handler can answer with an ordinary response; a failure after the first chunk faults the stream, as a failing producer aborts any stream. An empty stream is not checked in Bynk, since its length is not known up front: the platform answers a non-image with `InvalidImage`. The platform's 20 MB input limit still applies. The binding shares one pipeline builder between the two forms, looks the binding up outside the error mapping (a Worker without `[images]` faults), and rethrows the integration double's marked "not stubbed" fault. In a test `transformStream` faults until `stub Images.transformStream(_, _, _) returns …` replaces it.

**Consequences.** New names in `bynk.cloudflare` (`ImageStream`, the operation); a program that does not consume `Images` is unaffected. No grammar or checker change.
