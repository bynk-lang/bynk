---
title: Try an expression against your project
---
**Goal:** evaluate a snippet of Bynk in a unit's scope, against your real
declarations and your unsaved edits, and get its value back as JSON. It's what an
editor notebook runs for a cell.

`bynk eval` compiles the snippet as a test case for the unit you name, runs it
under the test runtime and prints the value of its last expression. It's an
explicit, local run of code you wrote, at the trust level of `bynk test`: nothing
is deployed and nothing reaches the network.

## 1. The project

A small shop of three units. `shop.money` is a commons with a refined type and a
function:

```text
commons shop.money

type Percent = Int where InRange(0, 100)

fn discount(cents: Int, off: Percent) -> Int {
  cents - cents * off / 100
}
```

`shop.catalog` is a context that exports a type:

```text
context shop.catalog

exports transparent { Sku }

type Sku = String where NonEmpty
```

`shop.orders` is a context that `uses` the money, `consumes` the catalogue, and
calls out over `Fetch`:

```text
context shop.orders

uses shop.money
consumes shop.catalog
consumes bynk { Fetch, Request, Method, Response }

type Order = {
  id: String,
  cents: Int,
}

fn total(o: Order, off: Percent) -> Int {
  discount(o.cents, off)
}

service notify {
  on call(url: String) -> Effect[Int] given Fetch {
    let res <- Fetch.send(Request {
      method: Post,
      url: url,
      contentType: None,
      authorization: None,
      headers: Map.empty(),
      body: None,
    })
    match res {
      Ok(r) => Effect.pure(r.status)
      Err(_) => Effect.pure(-1)
    }
  }
}
```

## 2. Evaluate an expression

Name the unit with `--unit` and the snippet with `--expr`:

```sh
bynk eval --unit shop.money --expr 'discount(2000, 25)'
```

```json
{"value":1500,"type":"Int","diagnostics":[]}
```

`value` is the wire shape of the last expression, the same JSON `Json.encode`
gives it, and `type` is its type. The snippet sees what a suite for that unit
sees: the unit's own declarations, without any `consumes` of its own.

Without `--expr`, the snippet is read from standard input, which is easier for
more than a line:

```sh
echo 'Percent.of(150)' | bynk eval --unit shop.money
```

A refused construction is a value, not a failure: the result's `value` is
`{"kind":"Err", …}` carrying the `ValidationError`, and `bynk eval` exits 0.

## 3. Carry earlier cells in as a preamble

Each evaluation is independent: nothing is kept from the last one. A client that
wants an earlier cell's bindings sends them again ahead of the snippet:

```text
let base = 2000
let off = Percent.of(25)
base * 2
```

## 4. Import what you use from other units

Imports are per file, so a snippet `consumes` what it uses from a consumed context,
as a suite file does. In `shop.orders`'s scope, `Sku` is not there until the
snippet asks for it:

```sh
bynk eval --unit shop.orders --expr 'Sku.of("a")'
```

```json
{"value":null,"type":null,"diagnostics":[{"line":1,"col":1,"severity":"error","code":"bynk.consumes.type_not_selected","message":"`Sku` is not in scope: this file does not `consumes shop.catalog`"}]}
```

Put the `consumes` first:

```text
consumes shop.catalog { Sku }
Sku.of("")
```

```json
{"value":{"error":{"field":"Sku","message":"must be non-empty","value":""},"kind":"Err"},"type":"Result[Sku, ValidationError]","diagnostics":[]}
```

A snippet's leading `consumes` and `stub` lines belong to the suite the snippet is
built into, and the rest is the case. Blank lines and `//` comments among them are
fine. A `consumes` after the first statement is a parse error, reported at its
line. Units used with `uses` (like `shop.money` above) are in scope already.

## 5. Stub a capability

A capability that reaches outside the process (`Fetch`, a database, a secret
store) has no real implementation here: the test runtime refuses it unless it's
stubbed, and the result says so. This is `shop.orders`, which consumes `Fetch`,
sending a request with no stub:

```sh
bynk eval --unit shop.orders --expr 'consumes bynk { Fetch, Request, Method, Response }
let res <- Fetch.send(Request { method: Get, url: "https://example.com", contentType: None, authorization: None, headers: Map.empty(), body: None })
res'
```

```json
{"diagnostics":[],"error":{"kind":"runtime","message":"Error: bynk: `Fetch.send` is not stubbed, and a test never reaches the network — add `stub Fetch.send(_) returns …`"}}
```

Stub it in the snippet's leading lines, as in a suite, and it answers:

```text
consumes bynk { Fetch, Request, Method, Response }
stub Fetch.send(_) returns Ok(Response { status: 204, headers: Map.empty(), body: "" })
let res <- Fetch.send(Request { method: Get, url: "https://example.com", contentType: None, authorization: None, headers: Map.empty(), body: None })
res
```

```json
{"value":{"kind":"Ok","value":{"body":"","headers":[],"status":204}},"type":"Result[Response, FetchError]","diagnostics":[]}
```

The unit has to consume the capability itself, as it does for the code the
snippet calls. `Clock`, `Random` and `Logger` run as the in-memory doubles every
test gets, so they need no stub.

To use what the unit's suite already declares (its `stub`s, and its helper `fn`s
and fixtures), pass `--suite` with the same unit as `--unit`. Given this suite:

```text
suite shop.orders {
  fn bulk(cents: Int) -> Order {
    Order { id: "bulk", cents: cents * 100 }
  }

  case "a bulk order costs more" {
    expect bulk(5).cents == 500
  }
}
```

```sh
bynk eval --unit shop.orders --suite shop.orders --expr 'total(bulk(5), 10)'
```

```json
{"value":450,"type":"Int","diagnostics":[]}
```

The snippet is then built inside that suite, so its `consumes`, `stub`s and `fn`s
apply. A suite whose own file doesn't compile is reported. Without `--suite`, the
project's suites are left out of the build altogether, so a broken suite elsewhere
doesn't stop you evaluating.

## 6. Unsaved edits

With `--sources-stdin`, the buffers on standard input win over the files on disk,
as for [`bynk compile`](/reference/tooling/bynk-cli/#bynk-compile):

```sh
echo '{"/work/shop/src/money.bynk": "commons shop.money\n…"}' \
  | bynk eval --sources-stdin --unit shop.money --expr 'discount(100, 10)'
```

Standard input then carries the buffers, so the snippet has to come from `--expr`.

## 7. When it doesn't check or doesn't finish

A snippet that doesn't type-check returns structured diagnostics at its own lines
and columns, and exits 1. For the snippet

```text
let a = 1
let x: Int = "no"
x
```

the result is:

```json
{"value":null,"type":null,"diagnostics":[{"line":2,"col":14,"severity":"error","code":"bynk.types.let_annotation_mismatch","message":"let binding's value has type `String`, but the annotation declares `Int`"}]}
```

A run that goes past the time limit is killed and reported as
`{"error":{"kind":"timeout", …}}`. The limit is 60 seconds; `--timeout SECONDS`
changes it.

See the [`bynk eval` reference](/reference/tooling/bynk-cli/#bynk-eval) for every flag and the
result's shape, and [Run your tests](/book/guides/testing/run-tests/) for the
runner it shares.
