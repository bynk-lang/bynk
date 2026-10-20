---
title: "`bynk.agents.non_zeroable_state_field`"
---
```text
[bynk.agents.non_zeroable_state_field] agent `Gauge` store cell `level` has no
defined zero value, so a fresh key cannot be initialised
```

## What it means

An agent's `store` field has a type with no zero value. Bynk initialises a
never-seen key's state automatically, so every field needs a well-defined
starting value. Types that have one include `Int` (`0`), `Bool` (`false`),
`String` (`""`), `Option[T]` (`None`), and records of zeroable fields. Types that
do **not** include opaque types, sum types (other than `Option`), and refined
types that exclude their zero.

```bynk,fail=bynk.agents.non_zeroable_state_field
type Level = Int where Positive

agent Gauge {
  key id: String
  store level: Cell[Level]   // Positive excludes 0 — no zero value

  on call read() -> Effect[Level] {
    level
  }
}
```

## Fix

- **Add an initialiser.** Give the field an explicit starting value with
  `= <value>`; any type becomes admissible, including sums (a state machine's
  initial state) and refined types:

  ```bynk,fragment
  store level: Cell[Level] = 1
  ```

  The initialiser must be a compile-time value; see
  [`bynk.agents.bad_state_initialiser`](/book/troubleshooting/agents-bad-state-initialiser/).

- **Use `Option` for "not set yet".** `None` is a valid zero and means "never
  set" (`store level: Cell[Option[Int]]`).
- **Relax the refinement** so the zero is admitted (e.g. `NonNegative` instead of
  `Positive`, since `0` satisfies `NonNegative`).

## Related

- [Build a stateful agent](/book/guides/agents-and-state/stateful-agent/)
- Reference: [agents](/book/reference/agents/)
- Explanation: [The agent model](/book/guides/agents-and-state/the-agent-model/)
