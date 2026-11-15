---
level: minor
changelog: `bynk client` generates `@raw` request bodies and the raw and streamed answers (`Raw`, `RawTagged`, `RawCreated`, `RawStreamed`, `RawError`, and `Streaming` as an async iterable of server-sent events) (#3508 slice BC4)
probes: ts_writes
---

## ADR: bynk-client-raw-bodies-and-streams
title: `bynk client`: raw bodies, raw answers and server-sent events
summary: A `@raw` body is the caller's bytes under the declared type, a raw answer hands over the response body untouched, and `Streaming` is an async iterable of the events the server frames

**Context.** BC1 to BC3 of #3508 left out the routes whose body Bynk does not own the form of: a `@raw` request body, the raw answers (`Raw`, `RawTagged`, `RawCreated`, `RawStreamed`, `RawError`) and the streamed `Streaming` answer. The proposal's Decision F fixed their shape and asked this slice to verify the event framing against the server, which it did by reading `sseResponse` in the runtime and running the generated reader against a real Worker and `workerd`.

**Decision.**

- *`@raw` request body.* `Uint8Array | ArrayBuffer | Blob | ReadableStream<Uint8Array>`, sent as given. No codec runs. The route's declared content type is set, replacing one configured for JSON; `*/*` names none, so the caller's own stands. A `ReadableStream` is sent with `duplex: "half"`, which `fetch` requires of a streamed upload; only some browsers can make one, and the reference says so.
- *Raw answers.* The member carries `contentType`, `body` (the response's own `ReadableStream`, so a download is not buffered), and `bytes()` and `text()` over that same body, and `etag` where the server can send one: a `string` for a `RawTagged`, `string | null` where a `Raw` can answer (a small `GET` gets a weak `ETag` from the server). The wire cannot tell one raw variant from another at a status (the runtime says so beside `RawTagged`), so every raw answer at a status is one member whose `kind` is the union of the variants that can make it. A status that also answers in JSON is told apart by content type: `application/json` is the JSON answer, anything else the raw one. `RawError` answers the status its handler names, which the wire peek reports only when it is a literal.
- *Server-sent events.* `Streaming` takes a `Stream[String]` (the checker requires it), so the proposal's decoding of events by the body codec does not apply: an event is a `string` and nothing is decoded. The server frames each element as one `data:` line per line of the element and a blank line, with no `event:`, `id:`, `retry:` or comments. The member is `events: AsyncIterable<string>`, read from the `fetch` body (an `EventSource` cannot send headers or a body) by a reader that follows the specification: CRLF, LF or CR line ends, `data:` with or without a space, comments and other fields ignored, an unfinished event dropped, a multi-byte character split across chunks kept whole. Breaking the loop or aborting the call cancels the body. A stream that fails after its status was sent cannot become a `CallFailure`, so the iteration throws an exported `StreamFailure` carrying the `reason`, the one place the client throws.
- *Known edges.* The server splits an element on `\n` only, so an element holding a bare `\r` is two lines to a spec reader and does not round-trip; an empty element is an event with empty data and does round-trip. Where the network cuts a streamed body is not the client's to promise, so tests compare joined text.
- *Verified.* The system tier runs the client against a real Worker in process and under `workerd`: uploads in each accepted form (a stream included), every raw answer, the events as the server frames them, an early `break`, and the reader over frames written by other servers and cut at awkward places.

**Size.** The reader and the raw handling are reached only through a route's `streamed` entry, so a client with no raw or streamed route does not carry them: a one-route client grows from 1443 to 1520 bytes gzipped (the `@raw` request path), measured as BC1's was, minified with esbuild and gzipped.

**Consequences.** Every `from http` route can be called except a raw `RequestQuery` or `RequestCookies` view and a signed route. The generated text of existing routes is unchanged apart from the runtime gaining the reader and the raw handling, so the goldens are blessed.
