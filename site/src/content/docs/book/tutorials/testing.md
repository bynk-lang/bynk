---
title: Test it
---
A language built around correctness should make tests easy, and Bynk builds
testing in: `suite`/`case` blocks, `expect`, value fabrication with `Val[T]`, and
per-seam test doubles with `stub`. In this chapter we test the shortener
from [Add a stateful agent](/book/tutorials/stateful-agent/) and meet each of those tools.

## Lay out a test project

Tests are ordinary `.bynk` files — a `suite` is a test wherever it lives.
Conventionally they sit in a `tests/` tree beside `src/`, declared with a
`bynk.toml` manifest:

```text
url-shortener/
├── bynk.toml
├── src/
│   ├── links.bynk
│   ├── registry.bynk
│   └── shortener.bynk
└── tests/
    └── registry.bynk
```

The manifest just names the project — the conventional `src/`+`tests/` layout
needs no `[paths]` config:

```toml
[project]
name = "url-shortener"
version = "0.1.0"
```

Move the three files from [Split into contexts](/book/tutorials/contexts/)
into `src/`. A `suite` names the unit it tests, so `tests/registry.bynk` — or a
`suite` block right inside `src/registry.bynk` — tests the `registry` context,
where the `Link` agent and the `create` service now live. A suite tests one
unit: the HTTP routes in `shortener` would get a suite of their own. (When you
build, the `suite` is stripped; only `bynk test` runs it.)

## Write a test and assert

A test file is a `suite` block naming its target, containing one or more named
cases. Inside a case, `expect` checks a predicate. Put this in
`tests/registry.bynk`:

```bynk,fragment
suite registry

case "a fresh code resolves to NotFound" {
	expect ShortCode.of("fresh2") is Ok(code)
	let link = Link(code)
	let outcome <- link.resolve()
	expect outcome is Err(_)
}

case "register then resolve returns the target" {
	expect ShortCode.of("reg001") is Ok(code)
	expect Url.of("https://example.com/page") is Ok(url)
	let link = Link(code)
	let _ <- link.register(url)
	let outcome <- link.resolve()
	expect outcome is Ok(view)
	expect view.target == url
}
```

A few things to notice. We address an agent by constructing it with a key —
`Link(code)` — and call its handlers on the result. Because handlers return an
`Effect`, we bind their results with `<-` rather than `=`. The first test proves
*fresh-state initialisation*: a code never registered reads `target: None`, so
`resolve` reports `NotFound`. The second proves state *persists* — we register,
then resolve and get the target back. Note `assert outcome is Err(_)`: `is`
matches a value against a pattern and yields a `Bool`, perfect for "this is an
`Err`, I don't care about the payload". And `expect … is Ok(code)` does double
duty: it checks the value is an `Ok` and keeps `code` bound for the rest of the
case, because a failed `expect` ends the case. (The older `match` with an
`Err(_) => expect false` arm still works.)

## Run the tests

Run the whole suite with `bynk test`:

```sh
bynk test .
```

`bynk` compiles the project (including the tests), type-checks the generated
TypeScript with `tsc`, and runs it with Node. You will need `tsc` and `node` on
your path. The output:

```text
Running tests...

registry:
  ✓ a fresh code resolves to NotFound
  ✓ register then resolve returns the target

2 passed, 0 failed.
```

`expect` is only valid inside a test case — using it elsewhere is a compile error
(`bynk.expect.outside_case`), so test-only checks can never leak into production
code.

## Fabricate values with `Val[T]`

Tests often need a value of some type without caring exactly what it is.
[`Val[T]`](/reference/glossary/#term-val) fabricates one. For a refined type
it produces a value that satisfies
the refinement; pass an argument to pin a specific one:

```bynk,fragment
case "a fabricated code is a valid ShortCode" {
  let code = Val[ShortCode]
  expect code == code
}

case "a pinned value takes the given value" {
  let code = Val[ShortCode]("abc123")
  expect code == code
}
```

`Val[ShortCode]` yields a valid `ShortCode`; `Val[ShortCode]("abc123")` pins it,
checked against the refinement at compile time. Like `expect`, `Val[T]` is
test-only (`bynk.val.outside_test` outside a test). Some types need a pin — a
`Matches`-refined string can't be fabricated blindly, so a bare `Val` of one is
rejected with `bynk.val.needs_pin`; pin it and you are fine.

To check a claim across a *range* of generated inputs rather than one fabricated
value, reach for a `property` and its `for all` — see the
[testing reference](/reference/testing/).

## Stub a collaborator with `stub`

The shortener's `create` service depends on the `CodeGen` capability from
[Reach out with capabilities](/book/tutorials/capabilities/) (it asks for it with
`given CodeGen`). When a case depends on what that collaborator *returns*,
override the seam with a `stub` clause — the capability, the method with an
argument pattern, and a value on the right:

```bynk,fragment
suite registry {
	stub CodeGen.next() returns "test01"

	case "create mints a code via the stubbed generator" {
		match Url.of("https://example.com") {
			Err(_) => expect false,
			Ok(url) => {
				let outcome <- create.call(url)
				expect outcome is Ok(_)
			},
		}
	}
}
```

The `stub CodeGen.next() returns "test01"` clause stands in for the real
`CodeGen` for these cases, so `create` mints the predictable `"test01"` instead of
whatever production would. The right-hand side is a *value* (or `fails`), never a
body — a double that needs logic is the signal to promote the case with `as
integration` and run the real collaborator. We call the service through
`create.call(url)` and assert it succeeded.

Run everything again:

```sh
bynk test .
```

```text
registry:
  ✓ a fresh code resolves to NotFound
  ✓ register then resolve returns the target
  ✓ create mints a code via the stubbed generator
  ✓ a fabricated code is a valid ShortCode
  ✓ a pinned value takes the given value

5 passed, 0 failed.
```

> Capabilities and `given`-based dependency injection are a topic in their own
> right; here we only need enough to stub one. See the
> [capabilities how-to guides](/book/guides/effects-and-capabilities/) for the full
> treatment, and [Test tiers](/book/guides/testing/integration/) for promoting a
> case to run its real collaborators, in one context or across the wire.

## What you have done

You laid out a test project, wrote `case`s with `expect`, ran them with
`bynk test`, fabricated values with `Val[T]`, and stubbed a collaborator with
`stub`. The shortener is now a tested system: a first compiled program grown
through an HTTP service, a data model, refined types, errors as values,
capabilities, a stateful agent and two contexts. One step is left: putting it
live.

➡️ **Next: [Deploy it](/book/tutorials/deploy/)**

---

*For the reasoning behind `Val[T]` and test isolation, see
[The testing philosophy](/book/concepts/testing/philosophy/). For exact rules,
see the [testing reference](/reference/testing/).*
