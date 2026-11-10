---
title: "Recursion and iteration"
---
Bynk has no loop statement. You repeat work in one of two ways: with the
collection kernel, or by having a function call itself. This page says which to
reach for, and what the language promises about the second.

## Reach for the kernel first

To visit every element of a collection, use the kernel: `fold`, `map`, `filter`,
`forEach` and the traverse family. They need no recursion, have no depth limit,
and say what you mean:

```bynk
commons totals {
	fn total(xs: List[Int]) -> Int {
		xs.fold(0, (acc, x) => acc + x)
	}
}
```

Recursion is for what the kernel does not cover: a loop whose number of steps
depends on what it finds (a parser, a state machine, a search that stops early,
a fetch that pages until a cursor runs out), and a structure that is itself
recursive, such as a tree.

## A self tail call is a loop

When a function's last act is to call itself, the call is a **self tail call**,
and Bynk guarantees it runs in constant stack, however many times it repeats.
You carry the running result in an accumulator parameter:

```bynk
commons loops {
	fn count(n: Int, acc: Int) -> Int {
		if n == 0 { acc } else { count(n - 1, acc + 1) }
	}
}
```

`count(1000000, 0)` returns `1000000`. The compiler emits the body as a loop and
turns the call into "set the parameters, go round again", so there is no stack to
overflow.

The guarantee holds for `Effect` functions as well. This runs a million steps in
constant stack, and no chain of pending promises grows behind it:

```bynk
commons loops {
	fn repeat(n: Int, step: Int -> Effect[()]) -> Effect[()] {
		if n <= 0 {
			Effect.pure(())
		} else {
			do step(n)
			repeat(n - 1, step)
		}
	}
}
```

The effects happen in the same order as they would without the loop.

## What counts as a tail call

A call is a tail call when nothing is left to do with its result. That is the
function body's final expression, and the final expression of each branch of an
`if`, each arm of a `match` (guarded arms too), and each block, that lead to it.

These are **not** tail calls, because something happens to the result:

```bynk,ignore
n + sumTo(n - 1)            // an operand
Some(find(rest))            // wrapped
Effect.pure(go(n - 1))      // wrapped
let r = go(n - 1)  r + 1    // bound by `let`, then used
go(n - 1)?                  // under `?`
xs.map((x) => go(x))        // inside a lambda
```

## What is not covered

The guarantee is for a function calling **itself**, directly. These recurse as
ordinary calls, and their depth is limited by the host's stack (which on Workers
is smaller than on Node), so keep them shallow or rewrite them as a self tail call
or a fold:

- **Non-tail recursion**, as above.
- **Mutual recursion**: `isEven` calling `isOdd` calling `isEven`, even though each
  call is in tail position.
- **A call through a function value.**
- **A generic function calling itself at a different type**, such as `f[A]` calling
  `f[List[A]]`.
- **A function with an `ensures` clause.** A postcondition relates one call's
  arguments to its result, and a loop has only one call to check, so the compiler
  leaves such a function as direct recursion rather than weaken the contract.
  Move the postcondition to a wrapper, or accept the host's depth limit. This is
  the same in the deploy build, where contracts are stripped, so the two builds
  behave alike.

A `requires` clause does not stop the rewrite: it is checked on every iteration,
against the arguments of the call that started it.

The depth of non-tail recursion is host-defined and the specification states no
number. Exceeding it is a fault, like any other.

## Each iteration has its own bindings

A function created inside the loop keeps the values of its own iteration, as it
would across separate calls:

```bynk
commons loops {
	fn adders(n: Int, acc: List[Int -> Int]) -> List[Int -> Int] {
		if n == 0 { acc } else { adders(n - 1, acc.append((x: Int) => x + n)) }
	}
}
```

`adders(3, [])` holds functions that add 3, 2 and 1.

**See also:** [Match on a list's shape](/book/guides/type-system/match-on-a-lists-shape/),
[Spec §5.2](/book/spec/static-semantics/#52-well-typedness) and
[§7.3.2](/book/spec/emission/#732-expressions).
