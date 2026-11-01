---
title: Generic code that crosses the boundary
---
**Read this when** you want a generic function to encode or decode JSON, or a
capability operation to take a typed value, instead of repeating `Json.encode` and
`Json.decode[T]` at every call site.

## Declare `Codable`

`Codable` holds for every type `Json.decode` accepts, so there is nothing to declare on
the type. Declare it on the parameter:

```bynk
commons guide.codec

type Point = { x: Int, y: Int }

fn roundTrip[T: Codable](v: T) -> Result[T, JsonError] {
  Json.decode[T](Json.encode(v))
}

fn encodeAll[T: Codable](xs: List[T]) -> String { Json.encode(xs) }

fn check(p: Point) -> Result[Point, JsonError] { roundTrip(p) }
```

The caller supplies the codec the compiler already generates for the concrete type, so
a refined type is validated on the way in exactly as at any other boundary. `List`,
`Option`, `Set`, `Map` and `Result` over a `Codable` parameter work. A generic type of
your own over a parameter (`Box[T]` inside `fn f[T: Codable]`) does not yet; it is
`bynk.generics.codec_generic_at_rigid`.

## Send a typed value to a queue

A capability operation can take a `Codable` parameter. The call site encodes it, so the
provider receives plain JSON and no codec:

```bynk,ignore
consumes bynk.cloudflare { Queue }

capability OrderEvents = Queue @name("order-events")

service api from http {
  on POST("/orders") (order: Order) -> Effect[HttpResult[String]] by Visitor given OrderEvents {
    let _ <- OrderEvents.sendValue(order)
    Ok("queued")
  }
}
```

`sendValue(order)` puts exactly the message `send(Json.encode(order))` puts on the
queue, so a consumer decodes it unchanged.

## What to know

- A `Codable` type parameter may not appear in an operation's return type
  (`bynk.generics.codable_op_return`): decoding a provider's reply is not supported yet.
- If you forget the bound, `bynk check --fix` adds it
  ([`bynk.generics.missing_codable_bound`](/book/troubleshooting/generics-missing-codable-bound/)).

## Related

- Reference: [Generics and bounds](/book/reference/generics/#codable)
- Guide: [Decode untrusted JSON into a typed value](/book/guides/type-system/decode-json/)
