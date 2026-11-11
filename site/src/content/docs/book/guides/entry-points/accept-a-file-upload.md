---
title: Accept a file upload
---
**Goal:** take a browser's multipart upload (a file and a few fields) as a typed
record, with limits on how much a client may send.

Mark the handler `@multipart` and declare its `body` as a record. A field of type
`File` is an uploaded part; the other fields are ordinary form fields, decoded and
validated as for [a form](/book/guides/entry-points/accept-a-form/). `File` is a record
`bynk` exports: `{ filename: Option[String], contentType: String, bytes: Bytes }`.

## Declare the limits and the route

A multipart route **must** bound its upload: a `@limit(maxBody: …)` on the handler, or a
`limits { maxBody: … }` on the service, is required (`bynk.http.multipart_body_unbounded`).

```bynk,fragment
consumes bynk { File }

type Upload = {
	title: String where NonEmpty && MaxLength(80),
	avatar: File,
}

service api from http {
	@multipart
	@limit(maxBody: 5000000, maxFile: 4000000)
	on POST("/avatar") (body: Upload) -> Effect[HttpResult[Int]] by v: Visitor {
		Created(body.avatar.bytes.length())
	}
}
```

- `maxFile` caps one file part and `maxParts` the number of parts (64 by default); past
  either the router answers `413` before the handler runs.
- A body that is not valid multipart is a `400` (`MalformedForm`), and a request that is
  not `multipart/form-data` is a `415`.
- A `File` may be alone, an `Option` (an optional file) or a `List` (several), one layer
  only. The name and content type are what the client claimed: never use the filename as
  a storage key, and do not trust the type as proof of the content.
- The upload is buffered in memory (up to the cap), then handed over as `Bytes`; pass it
  on, for example to R2, rather than keeping it.

## Test it

A case sends the typed record, and the system-tier driver builds the real `FormData`,
a `Blob` per file:

```bynk,fragment
suite upload.api as system {
	case "an avatar is accepted" {
		let r <- api.POST("/avatar", Upload {
			title: "me",
			avatar: File {
				filename: Some("me.png"),
				contentType: "image/png",
				bytes: Bytes.fromUtf8("png"),
			},
		})
		expect r is Handled(Created(_))
	}
}
```

**See also:** [Reference: multipart bodies](/book/reference/http/#multipart-bodies),
[the `File` type](/book/reference/types/#file).
