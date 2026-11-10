---
title: "Work with `Result` and optional values"
---
**Goal:** produce and consume `Result` (success or error) and `Option` (a value
or nothing).

Bynk has no exceptions and no `null`. Fallible operations return `Result[T, E]`;
possibly-absent values are `Option[T]`.

## Construct values

```bynk
commons demo {
	fn ok(n: Int) -> Result[Int, String] {
		Ok(n)
	}

	fn fail() -> Result[Int, String] {
		Err("nope")
	}

	fn wrap(n: Int) -> Option[Int] {
		Some(n)
	}

	fn empty() -> Option[Int] {
		None
	}
}
```

## Consume with `match`

```bynk
fn extract(o: Option[Int]) -> Int {
  match o {
    Some(n) => n
    None => 0
  }
}
```

The same form works for `Result`, with `Ok` and `Err` arms.

## Propagate errors with `?`

Inside a function that itself returns a `Result`, `?` unwraps an `Ok` or returns
early on an `Err`:

```bynk
commons demo {
	type Reps = Int where InRange(1, 100)

	fn doubled(n: Int) -> Result[Int, ValidationError] {
		let r = Reps.of(n)?
		Ok(r * 2)
	}
}
```

If `Reps.of(n)` is `Err(e)`, `doubled` returns `Err(e)` immediately; otherwise
`r` is the unwrapped value.

## Sequence fallible steps with `let … else` {#let-else}

`?` hands an `Err` straight back to the caller. When a failure needs handling
first, such as undoing an earlier step or choosing a response, matching each
result in turn pushes every later step one level deeper:

```bynk
commons demo {
	type Problem = enum { NoAddress, NoRoute }

	fn quote(address: Option[String], km: Result[Int, Problem]) -> Int {
		match address {
			None => 0,
			Some(a) => match km {
				Err(_) => 0,
				Ok(d) => d * 2,
			},
		}
	}
}
```

`let <pattern> = <value> else { … }` keeps the steps flat (the compiler hints
at this rewrite for a staircase two `match`es deep, and `bynk check --fix`
applies it; see [Flatten a staircase](/book/guides/type-system/match/#flatten-a-staircase)). When the value
matches the pattern, its names are bound for the rest of the block; when it
does not, the `else` block is what the block produces instead:

```bynk
commons demo {
	type Problem = enum { NoAddress, NoRoute }

	fn quote(address: Option[String], km: Result[Int, Problem]) -> Int {
		let Some(a) = address else { 0 }
		let Ok(d) = km else { 0 }
		d * 2
	}
}
```

To use the value that failed to match, write `else match` with arms for the
cases the pattern leaves:

```bynk
commons demo {
	type Problem = enum { NoAddress, NoRoute }

	fn quote(km: Result[Int, Problem]) -> Int {
		let Ok(d) = km else match {
			Err(NoAddress) => 0,
			Err(NoRoute) => 10,
		}
		d * 2
	}
}
```

The `else` stands in for the rest of **its own block**, so it must have the
type that block produces. It is not a `return`: inside an `if` branch it gives
that branch's value, and the function carries on. In an effectful body, use
`<-` to match an effect's result: `let Ok(id) <- Orders.open(cart) else { … }`.
See [Undo earlier steps when a later one fails](/book/guides/program-structure/consume-services/#undo-earlier-steps-when-a-later-one-fails)
for that form undoing calls to other contexts.

## Methods on `Option` and `Result` {#methods}

For the small cases a `match` is too much for, both types carry methods:

| `Option[T]` | |
| --- | --- |
| `map(f)`, `andThen(f)` | transform the value, or chain a step that may be absent |
| `getOrElse(x)`, `orElse(f)` | a default value, or a fallback `Option` (`f: () -> Option[T]`, run only on `None`) |
| `filter(p)` | keep the value only when `p` holds |
| `isSome()`, `isNone()` | test presence |
| `okOr(err)` | turn an absence into an `Err` |

| `Result[T, E]` | |
| --- | --- |
| `map(f)`, `andThen(f)`, `mapErr(f)` | transform the value or the error, or chain a fallible step |
| `getOrElse(x)` | a default value |
| `isOk()`, `isErr()` | test the outcome |
| `ok()` | keep the value, dropping the error (`Option[T]`) |

There is one name per operation: chaining a step is `andThen` (not `flatMap`)
on `Option` and `Result`.

## Related

- [Pattern-match with `match`](/book/guides/type-system/match/) ·
  [Narrow and bind with `is`](/book/guides/type-system/narrow-with-is/)
- [Define a refined type and validate untrusted input](/book/guides/type-system/define-and-validate/)
