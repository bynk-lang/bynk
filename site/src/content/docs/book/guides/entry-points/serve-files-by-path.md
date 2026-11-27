---
title: Serve files whose keys contain /
---
**Goal:** serve objects from an R2 bucket at `/files/<key>`, where a key may hold
`/` (`docs/2026/report.pdf`), with a few fixed paths beside them.

A `:name` segment matches one segment, so it can't take a key with a slash in it.
A **rest segment**, `*name` as a route's last segment, takes everything after the
segments before it. It is experimental, so opt in first, in `bynk.toml`:

```toml
[unstable]
allow = ["bynk.http.rest", "bynk.cloudflare.R2"]
```

## Declare the route

```bynk,ignore
context media

consumes bynk.cloudflare { R2 }

capability Files = R2

service files from http {
	on GET("/files/index.json") () -> Effect[HttpResult[String]] by v: Visitor {
		Ok("[]")
	}

	on GET("/files/*key") (key: String) -> Effect[HttpResult[Bytes]] by v: Visitor given Files {
		match key.percentDecode() {
			Some(k) => {
				let got <- Files.get(k.slice(1, k.length()))
				match got {
					Some(bytes) => Raw(bytes, "application/octet-stream")
					None => NotFound
				}
			}
			None => BadRequest("the key is not valid percent-encoding")
		}
	}
}
```

- **The rest arrives raw.** For `/files/docs/caf%C3%A9.txt`, `key` is
  `"/docs/caf%C3%A9.txt"`: still percent-encoded, starting with `/`. The handler
  decodes it with `percentDecode()` (`None` for a malformed escape, which this route
  answers `400`) and drops the leading `/` to get the R2 key `docs/café.txt`.
  Decoding is the handler's choice because some upstreams care about the
  difference between `%2F` and `/`; for a key store it rarely matters.
- **The fixed path still wins.** `/files/index.json` is a literal where the rest is a
  rest, so it answers that path whatever order the routes are written in.
- **An empty rest is allowed.** `/files` and `/files/` reach the rest route too, with
  `key` as `""` and `"/"`. Here both decode to an empty key, which R2 doesn't hold,
  so they are `404`. Add `on GET("/files")` if the bare prefix should list the bucket.
- **The path can't escape.** A request URL is normalised before routing, so `..`
  never reaches the rest, and the router answers `400` if a dot segment ever does.

## Test it

A case addresses the route by its pattern, then the rest:

```bynk,ignore
suite media {
	case "a key with a slash is served" {
		let _ <- Files.put("docs/a.txt", Bytes.fromUtf8("hello"))
		let r <- files.GET("/files/*key", "/docs/a.txt") by Visitor
		expect r is Raw(_, _)
	}
}
```

At the `system` tier the rest is sent as written, so a case can probe the encoding
(`"/docs/caf%C3%A9.txt"`) or an empty rest (`""`).

## A single-page app's fallback

`on GET("/*path")` at the root answers every `GET` no other route matches, which is
how a single-page app serves its shell for client-side routes. It never answers under
`/_bynk/`, and every more specific route keeps its own `405` and `Allow`.

**See also:** [Rest segments](/reference/http/#rest-segments),
[R2](/reference/bynk-capabilities/), [Testing](/reference/testing/).
