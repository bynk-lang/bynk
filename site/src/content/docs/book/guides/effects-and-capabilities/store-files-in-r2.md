---
title: Store files in R2
---
**Goal:** keep uploads, archives or exports as objects in a Cloudflare R2
bucket, and serve them back from a handler.

`R2` is **experimental**: it carries no compatibility promise yet. Opt in in
`bynk.toml`:

```toml
[unstable]
allow = ["bynk.cloudflare.R2"]
```

## Declare a bucket

A bucket is a named instance of `R2`; a bare `given R2` is refused. Consume the
adapter twice, once for the capability and once for the types its operations
use:

```bynk,fragment
context registry.store

consumes bynk.cloudflare
consumes bynk.cloudflare { R2 }

capability Archives = R2
```

`bynk deploy` creates the bucket, named `<worker>-archives`, and records it in
`bynk.deploy.lock`. Pin a bucket to a jurisdiction with
`capability Archives = R2 @jurisdiction("eu")`.

## Serve an object

`get` gives the object's bytes, or `None`. `Raw` sends them back with a content
type:

```bynk,fragment
service files from http {
  on GET("/dl/:key") (key: String) -> Effect[HttpResult[()]] given Archives {
    let found <- Archives.get(key)
    match found {
      Some(body) => Raw(body, "application/octet-stream")
      None       => NotFound
    }
  }
}
```

## Write once, or swap safely

`putWith` takes a condition and answers `None` when it fails, writing nothing.
`Absent` creates only, which is what immutable, content-addressed archives
want:

```bynk,fragment
let stored <- Archives.putWith(key, body, R2Put {
  contentType: Some("application/octet-stream"),
  metadata: Map.empty(),
  onlyIf: Absent,
})
```

To rewrite a file such as an index, read its `etag` with `head`, then write with
`onlyIf: EtagMatches(etag)`; if someone else wrote first you get `None` and can
read again.

## List

`list(prefix, cursor)` returns one page. Pass the page's `cursor` back for the
next one, until it is `None`:

```bynk,fragment
let page <- Archives.list(Some("sha256/"), None)
```

## Size

`get` and `put` hold the whole object in the Worker's memory, so keep each well
under 128 MB. Larger objects stream.

## Stream a large object

`getStream` gives the object's metadata and its body as a `Stream[Bytes]`,
read from R2 as the client pulls it. `RawStreamed` serves it without buffering:

```bynk,fragment
service files from http {
  on GET("/v/:key") (key: String) -> Effect[HttpResult[()]] given Archives {
    let found <- Archives.getStream(key)
    match found {
      Some(o) => RawStreamed(o.body, o.object.contentType.getOrElse("application/octet-stream"))
      None    => NotFound
    }
  }
}
```

The body can be read once. `putStream(key, body, size)` writes a stream, and R2
needs its length up front: a stream that holds more or fewer than `size` bytes
faults the write, and nothing is stored. Copying an object passes its own size:

```bynk,fragment
let found <- Archives.getStream(src)
match found {
  Some(o) => {
    let copied <- Archives.putStream(dst, o.body, o.object.size)
    Some(copied.size)
  }
  None => None
}
```

## Upload in parts

A request body is read whole, so a client sends an object too large for one
request as a multipart upload, a part a request. `createUpload` returns an
`R2Upload`, plain data the client keeps and sends back with each part:

```bynk,fragment
let u <- Archives.createUpload(key, R2UploadOptions { contentType: Some("video/mp4"), metadata: Map.empty() })
// later requests, each with the handle:
let part <- Archives.uploadPart(R2Upload { key: key, uploadId: id }, n, chunk)
// and finally, with every part's R2Part:
let o <- Archives.completeUpload(R2Upload { key: key, uploadId: id }, parts)
o.size
```

Number parts from 1. Every part but the last must be the same size, and R2
needs at least 5 MiB; it refuses a completion otherwise. `abortUpload`
discards an upload, and R2 discards one left incomplete for 7 days.

## Test it

Every test gets an in-memory bucket per instance, with the same conditions,
metadata and paging, so no stub is needed; `stub Archives.get(_) returns …`
still works when you want one. See
[the reference](/book/reference/bynk-capabilities/#object-storage-r2) for every
operation.
