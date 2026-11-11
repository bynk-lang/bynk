---
title: Accept an HTML form
---
**Goal:** take the fields of a browser's `<form method="post">` as a typed, validated
record, and see a bad field refused with its name.

A browser posts a form as `application/x-www-form-urlencoded`: flat `name=value` pairs.
Mark the handler `@form` and declare its `body` as a record, and the router decodes
the pairs into it before the handler runs, as it does a JSON body.

## Declare the record and the route

```bynk,fragment
type Email = String where Matches("^[^@]+@[^@]+$")
type Plan = enum { Free, Pro }

type Signup = {
	email: Email,
	plan: Plan = Free,
	tags: List[String] = [],
	remember: Bool = false,
}

service api from http {
	@form
	on POST("/signup") (body: Signup) -> Effect[HttpResult[String]] by v: Visitor {
		Created("welcome \(body.email)")
	}
}
```

- A repeated key (`tags=a&tags=b`) fills a `List` field.
- An unchecked checkbox sends nothing, so give a `Bool` a default
  (`remember: Bool = false`); a checked one sends `on`, which reads as `true`.
- An unfilled input sends `name=`, which is **absent**: an `Option` is `None` and a
  field with a default takes it. A plain `String` keeps `""`, so a `NonEmpty`
  refinement still fires.
- A field that fails its refinement is a `400` naming it (`body.email`), exactly as
  a bad JSON body is.
- A request that is not a form is a `415`: a `@form` route answers one media type.

## Test it

At the `system` tier a case sends the typed record, and the driver writes the real
encoding, so a mismatch fails the case:

```bynk,fragment
suite signup.api as system {
	case "a signup is accepted" {
		let r <- api.POST("/signup", Signup { email: "a@b.c", plan: Free, tags: [], remember: false })
		expect r is Handled(Created(_))
	}

	case "a bad address is refused" {
		let r <- api.POST("/signup", Wire("email=nope"))
		expect r is Rejected(RefinementViolation(_))
	}
}
```

## Read a form yourself

A handler that must keep the raw bytes (a signed webhook, say) takes `@raw`, verifies
the signature over the octets, and then reads them with the same mapping:

```bynk,fragment
fn read(text: String) -> Result[Signup, DecodeError] {
	Form.decode[Signup](text)
}
```

`Form.encode(signup)` writes the text back, for a test or an outbound request. A failure
is a `DecodeError` whose `path` is rooted at `$` (`$.email`).

**See also:** [Reference: form bodies](/book/reference/http/#form-bodies),
[`Form.decode` and `Form.encode`](/book/reference/types/#form-codec),
[Sending a form in a test](/book/reference/testing/#sending-a-form).
