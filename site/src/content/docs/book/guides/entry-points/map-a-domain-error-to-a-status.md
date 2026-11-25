---
title: "Map a domain error to a status once"
---
**Goal:** answer the same domain failure with the same HTTP status in every handler, by
writing the mapping once.

A service call that can fail returns a `Result` of your own error type, such as
`OrderError`. An HTTP handler has to turn that error into a status: `409` for "out of stock",
`404` for "no such order". Bynk does not map it for you. A status is chosen in the handler, on
the line where the call is made: `?` on a `Result` never picks one, and a table elsewhere that
did would hide a decision about the response away from the code that makes it. What you write
instead is a plain function, and each handler calls it.

## Write the mapping as a function

The function takes the error and returns an `HttpResult`. It is ordinary code, so it
is checked like any other: a `match` over the error type must cover every variant.

```bynk,fragment
type OrderError =
	| OutOfStock(sku: String)
	| Unknown(id: String)

fn orderFailure(e: OrderError) -> HttpResult[Order] {
	match e {
		OutOfStock(sku) => Conflict("\(sku) is out of stock")
		Unknown(_)      => NotFound
	}
}
```

## Use it at each call site

A `let … else match` unwraps the `Ok` and, on an `Err`, answers with the function. In a handler
the `else` block's value is the handler's result, so the mapped `HttpResult` is returned:

```bynk,fragment
on POST("/orders") (body: NewOrder) -> Effect[HttpResult[Order]] by v: Visitor given Orders {
	let Ok(order) <- Orders.place(body) else match { Err(e) => orderFailure(e) }
	Ok(order)
}

on GET("/orders/:id") (id: String) -> Effect[HttpResult[Order]] by v: Visitor given Orders {
	let Ok(order) <- Orders.find(id) else match { Err(e) => orderFailure(e) }
	Ok(order)
}
```

Both routes now answer an out-of-stock failure with the same `409` and the same message. Adding a
variant to `OrderError` makes `orderFailure` fail to compile until it says what the new case is,
so no handler can quietly answer it differently.

## Answer in a standard shape

With `errors { format: problem }` on the service (see [Problem
details](/reference/http/#problem-details)), the same function's variants that carry a body
render as RFC 9457 bodies (here, the `409`), with nothing in the mapping to change; a status
with no body, such as the `404`, still answers without one.

**See also:** [Reference: HTTP, answering another status](/reference/http/#answering-another-status),
[Handle an HTTP request](/book/guides/entry-points/http/).
