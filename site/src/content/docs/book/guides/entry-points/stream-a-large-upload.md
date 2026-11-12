---
title: Stream a large upload
---
**Goal:** accept an upload too big to hold in memory, such as an archive or a video, and
pass it straight on to storage, with a limit on how much a client may send.

A [`@raw`](/book/reference/http/#raw-request-bodies) handler whose `body` is a
`Stream[Bytes]` is handed the request body **as it arrives**, a chunk at a time, instead
of after it has been read whole. For a small upload that you want in memory, keep
`body: Bytes` (and see [Accept a file upload](/book/guides/entry-points/accept-a-file-upload/)
for a browser's multipart form).

## Declare the route, and its limit

A `@raw` route **must** bound its body: a `@limit(maxBody: …)` on the handler, or a
`limits { maxBody: … }` on the service. A streamed body is counted as the handler reads it.

```bynk,ignore
service archives from http {
  limits { maxBody: 5_000_000_000 }

  @raw(contentType: "application/zip")
  on PUT("/archives/:name") (name: String, headers: RequestHeaders, body: Stream[Bytes])
      -> Effect[HttpResult[()]] by u: User given Archives {
    …
  }
}
```

The `Stream[Bytes]` type is what turns streaming on. Without `@raw` it is
`bynk.http.stream_body_not_raw`, and a `Signature` actor or a sum of actors is refused on
the route, because a signature covers the whole body before the handler runs.

## Pass it to R2

`R2.putStream(key, body, size)` writes a stream of exactly `size` bytes, so read the size
from the request's `Content-Length` header:

```bynk,ignore
    match headers.get("content-length") {
      Some(length) => match Int.parse(length) {
        Some(size) => {
          let _ <- Archives.putStream(name, body, size)
          NoContent
        }
        None => BadRequest("Content-Length is not a number")
      }
      None => BadRequest("send a Content-Length")
    }
```

A stream with more or fewer bytes than `size` faults the write, and R2 stores nothing, so a
client that lies about its length cannot leave half an object behind. An upload sent
chunked has no `Content-Length`: it cannot go to `putStream`. Either answer with an error as
above, or pass the stream to
[`Fetch.sendStream`](/book/reference/bynk-capabilities/#streamed-bodies), which needs no
length, to forward it to another service.

## What the client sees past the limit

- A request that declares a `Content-Length` over the cap is answered `413` before the
  handler runs.
- A body that crosses the cap while the handler is reading it stops the read, and the router
  answers the same `413` (and `on rejected` renders it, as for any other). A sink that fails
  whole, like `putStream`, leaves nothing stored.
- A client that is still sending when the `413` goes out may see its connection reset
  instead of the response. The limit guarantees that the Worker stops reading, not what a
  given client's library reports.

## Things to keep in mind

- **A stream is read once.** Passing it to a sink, draining it with `collect()` or handing it
  to a response consumes it; a second read faults. It also cannot be passed to an agent or
  another service, since a `Stream` never crosses a boundary: store the result (an object key),
  not the bytes.
- **Read first, then answer.** Do not build the response from the request stream (an echo
  through `RawStreamed`): it failed under `wrangler dev`'s proxy and is unchecked on
  Cloudflare's edge ([#3396](https://github.com/bynk-lang/bynk/issues/3396)).
- **Retries.** A request body cannot be replayed, so a client that retries sends it again.
  If the upload must be safe to repeat, take an
  [`Idempotency`](/book/reference/bynk-capabilities/#the-idempotency-capability) key from the
  head of the request (a path segment or a header), not the body.
- **A browser** uploads a large file by `PUT`ting the file itself (`fetch(url, { method: "PUT",
  body: file })`) to a streamed `@raw` route. A `multipart/form-data` upload is still
  buffered: its parts are not streamed.

## Test it

A case passes a `Stream`, usually `Stream.of([…])`, in the body's position, after the path
parameter and the `headers: RequestHeaders` this route declares. At the `unit` and
`integration` tiers the stream reaches the handler as it is, so a case sets the
`Content-Length` itself and can see the upload reach R2:

```bynk,ignore
suite archives.api {
  case "an archive is stored" {
    let r <- api.PUT("/archives/:name", "a.zip", RequestHeaders.of(["Content-Length: 6"]),
                     Stream.of([Bytes.fromUtf8("PK"), Bytes.fromUtf8("data")])) by User("alice")
    expect r is NoContent
    expect Archives.putStream called
  }

  case "an upload with no Content-Length is refused" {
    let r <- api.PUT("/archives/:name", "a.zip", RequestHeaders.empty(),
                     Stream.of([Bytes.fromUtf8("PK")])) by User("alice")
    expect r is BadRequest(_)
  }
}
```

At `system` the driver sends a stream chunked, with no `Content-Length`, so the router's
counted read runs, and a case can drive the limit and the media type: a `Wire(…)` body is one
streamed chunk of `text/plain`, which this route refuses (a route declaring `"*/*"` takes it,
and a body past the cap is then `Rejected(PayloadTooLarge(_))`):

```bynk,ignore
suite archives.api as system {
  case "another media type is refused" {
    let r <- api.PUT("/archives/:name", "a.zip", RequestHeaders.empty(), Wire("not a zip")) by User("alice")
    expect r is Rejected(UnsupportedMediaType(_))
  }
}
```

The happy path of the `Content-Length` pattern above cannot be run at `system` today, because
the driver never sends a `Content-Length` for a stream: a case there reaches the handler's
`BadRequest`. Cover it at `unit` or `integration`, as above.

**See also:** [Reference: streamed request bodies](/book/reference/http/#streamed-request-bodies),
[Streams](/book/reference/types/#stream), [R2](/book/reference/bynk-capabilities/),
[Sending a streamed body](/book/reference/testing/#sending-a-streamed-body).
