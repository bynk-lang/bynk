---
title: Branch on the caller's country
---
**Goal:** answer differently by where the request came from, using what Cloudflare
already worked out, and have the handler still work where Cloudflare did not say.

Declare a `cf: RequestCf` parameter. Every field of it is optional, so match the one
you want.

## Read the country

```bynk,ignore
consumes bynk.cloudflare { RequestCf }

type Greeting = { text: String }

service api from http {
	on GET("/hello") (cf: RequestCf) -> Effect[HttpResult[Greeting]] by v: Visitor {
		match cf.country {
			Some("GB") => Ok(Greeting { text: "Hello" })
			_ => Ok(Greeting { text: "Hi" })
		}
	}
}
```

`RequestCf` is experimental, so add `bynk.cloudflare.RequestCf` to `[unstable] allow`
in `bynk.toml`. Naming it also locks the deployment unit to Cloudflare, which is the
point: no other platform has these facts.

## Write the test

The record literal is the builder, and its defaults make `RequestCf {}` a request with
no Cloudflare metadata:

```bynk,ignore
suite geo.api {
	consumes bynk.cloudflare { RequestCf }

	case "a caller in Britain" {
		let r <- api.GET("/hello", RequestCf { country: Some("GB") })
		expect r is Ok(Greeting { text: "Hello" })
	}

	case "no Cloudflare metadata" {
		let r <- api.GET("/hello", RequestCf {})
		expect r is Ok(Greeting { text: "Hi" })
	}
}
```

At the `system` tier the request is real, so assert nothing about a geographic value
you cannot control.

## Know what it is

These facts are derived from the caller's IP address, so treat them as personal data,
and as a best guess: a VPN or a proxy makes them wrong. [The reference](/book/reference/http/#request-cf)
lists every field.
