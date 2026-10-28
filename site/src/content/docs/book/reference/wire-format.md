---
title: Wire format (version 1)
---
<!-- GENERATED FILE — do not edit by hand.
     Source: bynk-check/src/wire_format.rs and wire_format/reference.rs, via bynkc/tests/wire_reference.rs.
     Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test it wire_reference:: -->

How one Bynk context calls another context's `on call` service over a
Cloudflare Service Binding: the request, the checks the callee makes, the
responses, the JSON encoding of every type that can cross, and the contract
hash. Code that is not Bynk can call a Bynk service by speaking this format.

This page is generated from the compiler. The header names, the check order
and the status codes are the constants the emitter builds the call dispatch
from; the encoding table is read from the compiler's wire model; and every
example below is run through the codecs the compiler generates for the
example context, by `bynkc/tests/wire_reference.rs`.

## Version and stability

This is wire format version **1**. It is versioned on its own, not
with the compiler: a compiler release changes it only by adding a new wire
version.

- **Within a version, changes are additive only.** A new status code, a new
  error `kind` or a new optional header may appear. A caller must treat an
  error `kind` it does not know as an error.
- **Anything else is a new version:** changing an encoding, a check, a status,
  an existing error body, or the contract hash's normal form or function.
- **The caller states its version** in `X-Bynk-Wire`; an absent header means
  `1`. **The callee accepts a range** (this compiler's callees accept
  `1`) and refuses any other version with the versions it accepts, so a
  version is retired after an overlap window, not on a flag day.
- **The signature is part of the version:** its headers, its signing input
  and its window. The keys are not; they belong to a deployment.

What the format does **not** cover: the TypeScript the compiler emits, the
in-memory representation of values, and the Worker's other entry points
(`from http` routes, events, queues). Those follow the compiler's own version.

## Request

```text
POST /_bynk/call/<service>
```

`<service>` is the service's name as declared (`service quote` is
`/_bynk/call/quote`). The host is ignored: a Bynk caller sends
`http://internal/_bynk/call/<service>` over the Service Binding.

| Header | Required | Meaning |
|---|---|---|
| `X-Bynk-Contract` | yes | The caller's hash of the service's contract, 16 lowercase hex digits. Must equal the callee's; a missing header does not. |
| `X-Bynk-Caller` | yes | The calling context's qualified name (`shop.orders`), which the handler reads as `c.identity`, or a foreign caller's actor name (`Legacy`). Selects the key the signature is checked under, so it is proved, not claimed. |
| `X-Bynk-Timestamp` | yes | When the caller signed the request, in whole seconds since the Unix epoch. |
| `X-Bynk-Signature` | yes | HMAC-SHA256 of the signing input under the edge's key, 64 lowercase hex digits. See *Authentication* below. |
| `X-Bynk-Wire` | no | The wire version the caller speaks, exactly the decimal digits of an accepted version (`1`, not `01` or `+1`). Absent means `1`. |
| `content-type` | no | Bynk callers send `application/json`. The callee does not check it. |

**Body.** JSON, encoding the handler's parameters:

- **no parameters:** any JSON value; Bynk callers send `{}`;
- **one parameter:** the parameter's value itself, not wrapped;
- **two or more:** an object keyed by parameter name. Unknown keys are ignored.

So `on call(line: Line, size: Qty)` takes `{"line": {…}, "size": 3}`, and
`on call(size: Qty)` takes `3`. Renaming or reordering parameters is a contract
change (see [Contract hash](#contract-hash)).

## Checks and responses

The callee makes these checks in this order. The first that fails answers the
request.

| # | Check | Status on failure | Body on failure |
|---|---|---|---|
| 1 | The method is `POST`. | `405` | empty body, `Allow: POST` |
| 2 | The body is at most 1 MiB (1048576 bytes). Read under the cap, before anything is hashed. | `413` | text `Payload Too Large` |
| 3 | `X-Bynk-Signature` verifies, under the key the callee holds for the edge from `X-Bynk-Caller`, over the signing input, and `X-Bynk-Timestamp` is within 300 seconds of the callee's clock. Compared in constant time. | `401` | text `Unauthorized` |
| 4 | `X-Bynk-Wire` is absent or names a version the callee accepts. | `400` | `{"kind":"UnsupportedWireVersion","accepted":[1]}` |
| 5 | The path names a service of the callee's context with an `on call` handler. | `404` | text `Not found` |
| 6 | The service's handler admits the caller the signature proved: a Bynk context by `Caller` (the default), a foreign caller only by its own `Foreign` actor. | `403` | text `Forbidden` |
| 7 | `X-Bynk-Contract` equals the callee's hash of that service. | `409` | `{"kind":"ContractMismatch","service":"…","expected":"…","actual":"…"}` |
| 8 | The body parses as JSON. | `400` | `{"kind":"MalformedJson","details":"Invalid request body"}` |
| 9 | The body decodes as the handler's parameters (see *Encoding*). | `400` | a `StructuralMismatch` or `RefinementViolation` body |

If every check passes, the handler runs:

- **`200`**, `content-type: application/json`, with the handler's return value
  encoded as below. A domain error is a `200` too: `Result[T, E]`'s `Err` is a
  value, not a status.
- **`500`**, text `Internal Server Error`, if the handler faults (including
  encoding a value the format cannot carry, such as an unsafe `Int`). The
  callee logs the fault; the body says nothing more. The response carries
  `X-Bynk-Fault: <context>`, the callee's qualified name, so the caller can
  say which Worker faulted. Only a `/_bynk/call/` fault is marked.

A status not listed here (a platform error between the Workers, for example)
is a transport failure. A Bynk caller reports a `409` `ContractMismatch` and a
marked `500` `CalleeFaulted` by name, and every other non-`2xx` as
`Transport`, with the status.

### Error bodies

Every JSON error body is an object with a `kind`:

| `kind` | Status | Fields |
|---|---|---|
| `UnsupportedWireVersion` | `400` | `accepted`: the wire versions the callee accepts |
| `ContractMismatch` | `409` | `service`, `expected` (the callee's hash), `actual` (the header as received) |
| `MalformedJson` | `400` | `details` |
| `StructuralMismatch` | `400` | `path`, `expected`, `actual`: the value had the wrong shape |
| `RefinementViolation` | `400` | `path`, `violation`: `{"field", "message", "value"}`, a value of the right shape broke a predicate |

`path` is a JSONPath-like location in the body: `$` is the whole body (one
parameter), `$.<param>` a parameter (several), then `.<field>` or `[<index>]`
inward. An unknown variant fails at the sum's own `path`, with
`expected: "sum variant kind"` and the `kind` that was sent as `actual`.

## Encoding

Each type has one JSON encoding, the same in both directions. A value is
decoded by checking its shape, then any predicates.

### Base types

| Type | JSON | Decode accepts | Encode |
|---|---|---|---|
| `Int` | number (an integer) | JSON number, and `Number.isSafeInteger` (whole, within ±(2^53 − 1)); otherwise `StructuralMismatch` (`expected: "safe integer"`) | an unsafe value throws in the sender (a `500`), never a lossy number |
| `Float` | number (a number) | JSON number, and finite; otherwise `StructuralMismatch` (`expected: "finite number"`) | a non-finite value throws in the sender |
| `Bool` | boolean (`true` or `false`) | JSON boolean | as is |
| `String` | string (a string) | JSON string | as is |
| `Bytes` | string (standard base64 with padding (RFC 4648 §4)) | valid base64; anything else is `StructuralMismatch` (`expected: "base64 string"`) | as is |
| `Duration` | number (whole milliseconds) | JSON number, and `Number.isSafeInteger` (whole, within ±(2^53 − 1)); otherwise `StructuralMismatch` (`expected: "safe integer"`) | an unsafe value throws in the sender (a `500`), never a lossy number |
| `Instant` | number (milliseconds since the Unix epoch) | JSON number, and `Number.isSafeInteger` (whole, within ±(2^53 − 1)); otherwise `StructuralMismatch` (`expected: "safe integer"`) | an unsafe value throws in the sender (a `500`), never a lossy number |
| `()` | `null` | anything; the value is not inspected | `null` |

`Int`, `Duration` and `Instant` are limited to JavaScript's safe-integer range
on the wire, so every JSON parser reads them exactly.

### Composite types

| Type | JSON |
|---|---|
| record | an object keyed by field name. A missing field decodes as its default when it has one, as `None` when it is an `Option`, and otherwise fails. Unknown keys are ignored. |
| sum (`enum` included) | an object with `"kind"`: the variant's name, beside the variant's payload fields: `{"kind":"Circle","radius":0.5}`, `{"kind":"Dot"}` |
| `Result[T, E]` | `{"kind":"Ok","value":<T>}` or `{"kind":"Err","error":<E>}` |
| `Option[T]` | `{"kind":"Some","value":<T>}` or `{"kind":"None"}`. A sender may also send `null` or leave the field out for `None`, or send the bare value for `Some`; Bynk always encodes the tagged form |
| `List[T]` | an array |
| `Map[K, V]` | an array of `[key, value]` pairs, in insertion order |
| refined type (`type Qty = Int where NonNegative`) | its base type's encoding. The decoder checks the predicates, refusing with `RefinementViolation` |
| opaque type | its base type's encoding. A caller cannot see the predicate, so only the owning context checks it |
| inline field refinement (`qty: Int where Positive`) | the field's encoding. The decoder checks the predicates, like a refined type's |
| generic instantiation (`Page[Line]`) | the declaration's encoding with the arguments substituted |

The in-memory discriminant of a Bynk sum is `tag`; on the wire it is
`"kind"`. The encoding never depends on the in-memory layout.

### Examples

Each row is decoded by the codec the compiler generates for `wire.sample`
below, then encoded again. Rejections show the error's `kind` and `path`.

```bynk
context wire.sample

type Qty = Int where NonNegative

type Shape =
  | Circle(radius: Float)
  | Dot

type Line = {
  sku: String,
  qty: Int where Positive,
  wait: Duration,
  delay: Option[Duration],
  at: Instant,
  blob: Bytes,
  note: Option[String],
  tags: List[String],
  stock: Map[String, Int],
  rush: Bool,
}

service quote {
  on call(line: Line, size: Qty) -> Effect[Result[Shape, String]] {
    Ok(Dot)
  }
}

type Tagged[T] = {
  n: Int where Positive,
  v: T,
}

service who {
  on call(t: Tagged[String]) -> Effect[Result[String, String]] by c: Caller {
    Ok(c.identity)
  }
}

service ping {
  on call() -> Effect[Result[(), String]] {
    Ok(())
  }
}

actor Legacy { auth = Foreign(secret = "LEGACY_CALL_KEY") }

service legacy {
  on call(n: Int) -> Effect[Result[Int, String]] by Legacy {
    Ok(n)
  }
}
```

| Type | JSON sent | Outcome | Note |
|---|---|---|---|
| `Line` | `{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"Some","value":250},"at":1767225600000,"blob":"aGk=","note":{"kind":"Some","value":"gift"},"tags":["new"],"stock":[["lon",3]],"rush":false}` | accepted; encodes back the same | Every field present. A record is an object keyed by field name. |
| `Line` | `{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true,"extra":1}` | accepted; encodes back as `{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"None"},"at":1767225600000,"blob":"","note":{"kind":"None"},"tags":[],"stock":[],"rush":true}` | An absent `Option` field is `None`. An unknown key is ignored. |
| `Line` | `{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"","note":"gift","tags":[],"stock":[],"rush":true}` | accepted; encodes back as `{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"None"},"at":1767225600000,"blob":"","note":{"kind":"Some","value":"gift"},"tags":[],"stock":[],"rush":true}` | A bare value where an `Option` is expected is `Some`. |
| `Line` | `{"sku":"A-1","qty":0,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}` | `RefinementViolation` at `$.qty` | An inline field refinement (`qty: Int where Positive`) is checked. |
| `Line` | `{"sku":"A-1","qty":1.5,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}` | `StructuralMismatch` at `$.qty` | An `Int` is a whole number. |
| `Line` | `{"sku":"A-1","qty":2,"wait":1.5,"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}` | `StructuralMismatch` at `$.wait` | A `Duration` is whole milliseconds. |
| `Line` | `{"sku":"A-1","qty":2,"wait":1500,"delay":{"kind":"Some","value":0.5},"at":1767225600000,"blob":"","tags":[],"stock":[],"rush":true}` | `StructuralMismatch` at `$.delay.value` | So is a `Duration` inside an `Option`, a `List` or a variant. |
| `Line` | `{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"%%","tags":[],"stock":[],"rush":true}` | `StructuralMismatch` at `$.blob` | `Bytes` is padded base64. |
| `Line` | `{"sku":"A-1","qty":2,"wait":1500,"at":1767225600000,"blob":"","tags":[],"stock":{"lon":3},"rush":true}` | `StructuralMismatch` at `$.stock` | A `Map` is an array of pairs, never an object. |
| `Qty` | `-1` | `RefinementViolation` at `$` | A named refined type is its base value, checked against its predicates. |
| `Tagged[String]` | `{"n":0,"v":"x"}` | `RefinementViolation` at `$.n` | A generic record's inline refinements are checked in every instantiation. |
| `Shape` | `{"kind":"Circle","radius":0.5}` | accepted; encodes back the same | A variant's payload fields sit beside `kind`. |
| `Shape` | `{"kind":"Circle","radius":1e999}` | `StructuralMismatch` at `$.radius` | A `Float` is finite; `1e999` parses as infinity. |
| `Shape` | `{"kind":"Square"}` | `StructuralMismatch` at `$` | An unknown variant is refused. |
| `Result[Shape, String]` | `{"kind":"Ok","value":{"kind":"Dot"}}` | accepted; encodes back the same |  |
| `Result[Shape, String]` | `{"kind":"Err","error":"sold out"}` | accepted; encodes back the same | A domain error is a `200` carrying `Err`. |
| `Result[(), String]` | `{"kind":"Ok","value":null}` | accepted; encodes back the same | `()` is `null`. |

## Contract hash

`X-Bynk-Contract` carries a hash of the service's signature, so a callee can
refuse a caller that was compiled against a different version of it (`409`).
It detects skew; it is not a credential and not a security control.

**Normal form.** One line per service:

```text
<service>(<param>: <type>, …) -> <type>
```

- Parameters keep their names and order. `Effect[T]` is written as `T`.
- Base types are their names; `()` is `()`; `List[T]`, `Option[T]`,
  `Map[K, V]`, `Result[T, E]` keep their arguments in order.
- A named type is its name followed by its structure, so both a rename and a
  change of shape change the hash:
  - a record is `Name{f: T, …}`, each field written `f: T` (or
    `f: T where P, …` with an inline refinement), the fields sorted by that
    text's UTF-8 bytes and joined with `, `;
  - a sum is `Name|A|B(f: T, …)`: each variant written as its name, followed
    by its payload fields in parentheses if it has any, each written `f: T`
    and sorted by that text's UTF-8 bytes (the wire keys a payload by field
    name, so a renamed field changes the hash and a reordered one does not),
    the variants sorted by that text's UTF-8 bytes, each preceded by `|`;
  - a refined type is its name and base run together, then its predicates:
    `type Qty = Int where NonNegative` is `QtyInt where NonNegative` (with no
    predicates, `QtyInt ` with a trailing space);
  - an opaque type is `NameBase opaque`, without its predicates;
  - a generic instantiation is the declaration's form with the arguments
    substituted, followed by `[A, …]`;
  - a type met again inside its own expansion is `@Name`.
- Predicates are a set: written as below, sorted by their UTF-8 bytes, with
  duplicates removed, and joined with `, `. They are `Positive`,
  `NonNegative`, `InRange(a, b)`, `InRangeF(a, b)`, `MinLength(n)`,
  `MaxLength(n)`, `Length(n)` and `Matches("…")`; `NonEmpty` is written
  `MinLength(1)`. Bounds are written as decimal numbers in their shortest
  round-trip form (`01` is `1`). A `Matches` pattern is quoted, with `\`, `"`
  and control characters backslash-escaped.
- No spaces other than those shown; no doc comments, defaults, `given` or
  `by` clauses.

**Hash.** FNV-1a, 64-bit, over the normal form's UTF-8 bytes: start from the
offset basis `0xcbf29ce484222325`; for each byte, XOR it in, then
multiply by the prime `0x00000100000001b3` modulo 2^64. The result is
written as 16 lowercase hex digits.

For `wire.sample` above, each service's normal form, byte for byte, and its
hash:

`quote`, hash `088187a243c77777`:

```text
quote(line: Line{at: Instant, blob: Bytes, delay: Option[Duration], note: Option[String], qty: Int where Positive, rush: Bool, sku: String, stock: Map[String, Int], tags: List[String], wait: Duration}, size: QtyInt where NonNegative) -> Result[Shape|Circle(radius: Float)|Dot, String]
```

`who`, hash `7741a7d724fd9ed7`:

```text
who(t: Tagged{n: Int where Positive, v: String}[String]) -> Result[String, String]
```

`ping`, hash `619a25c88aaf9af8`:

```text
ping() -> Result[(), String]
```

`legacy`, hash `60d1a63e5bbeb220`:

```text
legacy(n: Int) -> Result[Int, String]
```

| Changes the hash | Does not |
|---|---|
| the service's name; a parameter's name, order or type; the return type | field order; variant order; predicate order or duplicates |
| a field added, removed, renamed or retyped; a variant added, removed, renamed or re-shaped | an opaque type's predicates |
| a refined type's or an inline field refinement's predicates | doc comments; `given`; `by c: Caller` |
| renaming a named type | field defaults |

The normal form and the hash function are part of the wire version: a client
built once embeds hashes, so they change only with a new wire version.

## Authentication

Every call is signed. The callee refuses a call whose signature does not verify
with a bare `401`, before it looks at anything but the method and the body's
size, so a sender without a key learns nothing: not which services exist, not
a contract hash.

**Keys.** One key per *edge*, a caller and a callee. The callee holds it as
`BYNK_CALL_FROM_<CALLER>` and the caller as `BYNK_CALL_TO_<CALLEE>`, where
`<CALLER>` and `<CALLEE>` are the qualified context names upper-cased with each
dot written `__` (`shop.orders` → `BYNK_CALL_FROM_SHOP__ORDERS`, `shop.payment` →
`BYNK_CALL_TO_SHOP__PAYMENT`). A name the key can be named from is dot-separated segments of
lowercase letters and digits, each starting with a letter, with single
underscores between runs. `bynk deploy` mints each key and sets it on both
Workers; `bynk dev` uses a fixed local key.

**Signing.** The caller sends `X-Bynk-Caller` (its qualified name),
`X-Bynk-Timestamp` (whole seconds since the Unix epoch) and `X-Bynk-Signature`:
HMAC-SHA256, under the edge's key, of the *signing input*, written as 64
lowercase hex digits. The signing input is these fields, one per line,
separated by `\n` with no trailing newline:

1. `BYNK-CALL-1`
2. the caller, as sent in `X-Bynk-Caller`
3. the callee's qualified context name
4. the method
5. the URL's pathname exactly as sent, with no query string and no
   percent-normalisation
6. `X-Bynk-Contract` as sent, empty when absent
7. `X-Bynk-Wire` as sent, `1` when absent
8. `X-Bynk-Timestamp` as sent
9. the SHA-256 of the body's bytes, 64 lowercase hex digits

For example, `shop.orders` calling `authorise` on `shop.payment` with an empty
body signs:

```text
BYNK-CALL-1
shop.orders
shop.payment
POST
/_bynk/call/authorise
0123456789abcdef
1
1760000000
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

**Verifying.** The callee reads the body under its cap (1 MiB, 1048576
bytes, else `413`), picks the key it holds for the edge from `X-Bynk-Caller`,
and refuses (`401`) when it holds none, when `X-Bynk-Timestamp` is more than
300 seconds from its clock either way, or when the
signature does not match, compared in constant time. So:

- **A key's holder can claim only its own name.** A caller's name is the
  handler's `c.identity`, proved by the key, not claimed.
- **A key binds one callee.** The callee's name is signed, so a request cannot
  be replayed against another callee, even one that holds the same key.
- **Every header the callee acts on is signed**, so none can be changed in
  flight: not the contract, not the wire version.
- **A captured call can be replayed inside the window.** The timestamp bounds
  replay; nothing stops it within those 300 seconds. An
  `on call` handler with side effects that must happen once should make itself
  idempotent (the `Idempotency` capability).
- **An undeclared caller cannot call.** Without a key on the callee there is
  nothing to verify against, whether or not the handler reads its caller.

### Foreign callers

Code that is not Bynk calls a context the same way, once the context declares
it as a **foreign caller**:

```text
actor Legacy { auth = Foreign(secret = "LEGACY_CALL_KEY") }

service legacy {
  on call(n: Int) -> Effect[Result[Int, String]] by Legacy { Ok(n) }
}
```

The foreign caller sends its actor's name (`Legacy`) as `X-Bynk-Caller` and
signs exactly as above, under the declared secret, which the callee holds as
`LEGACY_CALL_KEY`. `bynk deploy` mints that secret and prints the
`wrangler secret put` that hands it to the foreign Worker's operator. An
actor's name starts with a capital letter, so it is never a context's: a
foreign caller is checked under its own secret and can never pass for a Bynk
context, nor a context for it.

A verified caller still has to be one the service's handler admits. A Bynk
context is admitted by `Caller` (the default, with no `by`), a foreign caller
only by its own actor; `by who: Caller | Legacy` admits both and tells them
apart. Anyone else gets a bare `403`.

A TypeScript Worker need not speak any of this by hand. For each context with
a foreign caller the build emits a typed client, `bynk-client.ts`, which
signs, stamps the contract hash and encodes exactly as this page describes;
see [TypeScript client](/book/reference/typescript-client/).
