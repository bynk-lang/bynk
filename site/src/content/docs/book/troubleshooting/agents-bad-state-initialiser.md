---
title: "`bynk.agents.bad_state_initialiser`"
---
```text
[bynk.agents.bad_state_initialiser] state field initialiser must be a static value of type `Int` (got `Light`)
```

## What it means

An agent state field's initialiser (`field: T = <value>`) is not a valid
**static value of the field's type**. Two cases trigger it:

- **Not static** — the initialiser references `self`, a parameter, a capability,
  or any runtime value. The initial state is the value a fresh key gets *before*
  any handler runs, so it cannot depend on runtime input.
- **Type mismatch** — the initialiser's type doesn't match the field (e.g. a
  variant of the wrong sum, or a literal of the wrong base type).

```bynk,fragment
store count: Cell[Int] = Red   // Red is a variant of Light, not an Int
```

## Fix

Use a compile-time value of the field's type:

- a literal (`3`, `"x"`, `true`) — admitted against a refined type if needed;
- a sum variant (`Pending`), `Some`/`None`/`Ok`/`Err`, or a record literal;
- `T.unsafe(lit)` for an opaque type defined in this context.

```bynk,fragment
store count:  Cell[Int]         = 0
store status: Cell[OrderStatus] = Pending
```

## Related

- Reference: [Agents](/book/reference/agents/) — state initialisation.
- [Model an agent as a state machine](/book/guides/agents-and-state/state-machine/)
