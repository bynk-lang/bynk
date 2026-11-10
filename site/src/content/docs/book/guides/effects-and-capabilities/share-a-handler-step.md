---
title: Share a handler step that logs or queries
---
**Goal:** factor a step that several handlers repeat (query, log the failure, answer
`500`) into one function, without passing the capability in by hand at every call.

## Declare `given` on a context function

A function in a **context** may declare `given`, after its return type. The function
uses those capabilities exactly as a handler does:

```bynk
context shop.audit

capability Logger {
	fn error(message: String) -> Effect[()]
}

provides Logger = NullLogger {
	fn error(message: String) -> Effect[()] {}
}

fn orZero(read: Result[Int, String]) -> Effect[Int] given Logger {
	match read {
		Ok(n) => Effect.pure(n),
		Err(why) => {
			let _ <- Logger.error("could not read: \(why)")
			Effect.pure(0)
		},
	}
}

service api {
	on call(n: Int) -> Effect[Int] given Logger {
		orZero(Ok(n))
	}
}
```

The function may be generic, recursive, or call other `given` functions. A lambda in
its body reaches the same capabilities.

## Declare what the callee declares

A call to a `given` function is legal only where the caller has those capabilities
in its own `given`. The rule is checked at every call, so it holds along the whole path:
`service → helper → helper`. Each signature on the path lists what it needs, so a
capability never appears in a body without appearing in the handler that reaches it.

Calling `orZero` from a handler that does not declare `Logger` is
`bynk.given.missing_for_call`. The error names the capabilities that are missing, and
the quick fix adds them to the caller's `given`.

## What stays the same

- Stubs and `expect Logger.error called …` work through the function: they apply to
  the capability, not to the call site.
- A `case` in a suite may call the function directly. It gets the target's capabilities
  as its handlers do.
- The function is not shared across contexts. A `commons` function cannot declare
  `given` (`bynk.given.fn_outside_context`): the contexts that use it may consume
  different capabilities. There, pass the operation in as a function value
  (`log: (String) -> Effect[()]`) and give it `(m) => Logger.error(m)` from the handler.
- Only a free function that returns `Effect[T]` may declare `given`; a method cannot,
  and `Events` cannot be named (`bynk.given.fn_unsupported`). Emit events from the handler.

**See also:** [Reference — Capabilities in functions](/book/reference/capabilities/#capabilities-in-functions).
