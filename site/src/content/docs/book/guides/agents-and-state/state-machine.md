---
title: Model an agent as a state machine
---
**Goal:** give an agent a lifecycle — `Pending → Placed`, `Open → Closed` — where
the current state is a sum, the start state is explicit, and transitions are
checked.

## Make the state a sum with an initial variant

A `store` field can be a sum type as long as it declares its **initial state**
with an initialiser:

```bynk
context orders

type OrderStatus = enum { Pending, Placed, Cancelled }
type OrderError  = enum { AlreadyPlaced, AlreadyCancelled }

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending   // the state machine; starts Pending
  store items:  Cell[Int]                      // extended state; implicit zero 0

  on call place() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Placed
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }

  on call cancel() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Cancelled
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }
}
```

A fresh `Order` key starts at `Pending` — not `None`. The sum *is* the state, so
there is no separate "uninitialised" case to handle.

```mermaid
stateDiagram-v2
  [*] --> Pending
  Pending --> Placed: place() succeeds
  Pending --> Cancelled: cancel() succeeds
  Placed --> Placed: place()/cancel() returns AlreadyPlaced
  Cancelled --> Cancelled: place()/cancel() returns AlreadyCancelled
```

*The sum-typed `status` is the machine; each handler only advances from
`Pending`, and the exhaustive `match` makes the compiler check every case.*

Text equivalent: a fresh `Order` starts at `Pending`. `place()` advances
`Pending → Placed` and `cancel()` advances `Pending → Cancelled`, each returning
`Ok`; from any other state both return an error (`AlreadyPlaced` from `Placed`,
`AlreadyCancelled` from `Cancelled`), leaving the state unchanged. `Placed` and
`Cancelled` are terminal — no handler leaves them.

In VS Code, the `Show State` lens on `agent Order` draws this diagram from the
handlers themselves, with a table of what each handler does from each state
(see [the extension's agent state view](/docs/tooling/vscode-bynk/)).

## Read the state, then transition

- **Read** the current state by matching on its bare name: `match status { … }`.
  The match is exhaustive, so adding a state forces every handler to consider it.
- **Transition** by assigning a new state: `status := Placed` moves the field; any
  other `store` fields are untouched.
- **Guard** a transition by handling the wrong states explicitly — above, `place`
  succeeds only from `Pending` and returns an error otherwise.

## Initial values for any field

The same `= value` initialiser gives any `store` field a starting value — a
non-zero default or a refined type that has no implicit zero:

```bynk
type Level = Int where Positive

agent Gauge {
  key id: String

  store level:   Cell[Level] = 1   // refined; 0 would violate Positive
  store retries: Cell[Int]   = 3   // a non-zero default

  on call peek() -> Effect[Result[(), String]] {
    Ok(())
  }
}
```

An initialiser must be a compile-time value (a literal — including one admitted to
a refined type — a variant, a record, or, for an opaque type, `T.unsafe(lit)`); it
can't reference `self`, parameters, or capabilities.

## Related

- Reference: [Agents](/book/reference/agents/) — state initialisation and the
  full state-machine rules.
- Tutorial: [Add a stateful agent](/book/tutorials/05-stateful-agent/).
