---
title: Compose a provider from other capabilities
---
**Goal:** build one adapter out of others — a `Payments` provider that uses
`Http` and `Logger` — instead of duplicating that wiring in every handler.

## Declare the provider's dependencies with `given`

A `provides` block may carry a `given` clause, exactly like a handler. The
provider's bodies can then call the listed capabilities:

```bynk
context payment

capability Logger { fn info(message: String) -> Effect[()] }
capability Http   { fn post(path: String) -> Effect[()] }
capability Charge { fn run() -> Effect[()] }

provides Logger = ConsoleLogger {
  fn info(message: String) -> Effect[()] { Effect.pure(()) }
}

provides Http = FetchHttp {
  fn post(path: String) -> Effect[()] { Effect.pure(()) }
}

provides Charge = HttpCharge given Http, Logger {
  fn run() -> Effect[()] {
    let _ <- Logger.info("charging")
    let _ <- Http.post("/charge")
    Effect.pure(())
  }
}
```

`Charge` depends on `Http` and `Logger`. A handler that uses `Charge` never sees
that — it just declares `given Charge`, and the composition root supplies the
rest.

## The rules

- A capability you call in a provider body must be in that provider's `given`
  (`bynk.given.undeclared_capability`).
- A provider that calls an agent handler must list every capability that
  handler needs: its own `given`, and the `given` of any agent handler it
  calls in turn (`bynk.provider.agent_call_undeclared_capability`). A service
  or agent handler doesn't have to. Its `given` is widened to cover the agent
  handlers it calls. A provider is built with exactly its `given`, so it can
  hand the agent nothing else.
- Each `given` name must be a declared capability
  (`bynk.given.unknown_capability`).
- A capability can't depend on itself, directly or transitively
  ([`bynk.provider.dependency_cycle`](/book/troubleshooting/provider-dependency-cycle/)).

## How it's wired

The providers form a dependency graph. The generated `compose` instantiates them
in dependency order and injects each one's dependencies — `Logger` and `Http`
first, then `new HttpCharge({ Http, Logger })`. You write the graph; the compiler
orders it.

## Related

- Reference: [Capabilities & providers](/book/reference/capabilities/).
