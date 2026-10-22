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
    match Archives.get(key) {
      Some(body) => Raw(body, "application/octet-stream")
      None       => NotFound("no such archive")
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

Objects are read and written whole and held in the Worker's memory, so keep each
well under 128 MB. Streaming larger objects is a later increment.

## Test it

Every test gets an in-memory bucket per instance, with the same conditions,
metadata and paging, so no stub is needed; `stub Archives.get(_) returns …`
still works when you want one. See
[the reference](/book/reference/bynk-capabilities/#object-storage-r2) for every
operation.
