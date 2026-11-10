---
title: The agent model
---
An **agent** is Bynk's unit of state. It is a named thing, identified by a key,
that owns some state and exposes handlers to read and change it. This page
explains what that means and why agent state must be
*[zeroable](/book/reference/glossary/#term-zeroable)*.

## What an agent is

Most of a Bynk program is stateless: functions and
[services](/book/reference/glossary/#term-service) transform inputs into outputs. An agent is the deliberate exception — the place where something is
*remembered* between calls.

Each agent has a **key**. Two calls naming the same key address the same logical
instance with the same state; different keys are independent. A `Counter` keyed by
`CounterId` is really a whole family of counters, one per id, each with its own
`count`. This maps directly onto the runtime: on the `workers` target an agent
becomes a Cloudflare Durable Object, where the key selects the object instance.

State lives in `store` fields: a handler reads a field by its bare name and writes
it with `:=`. The writes are [committed](/book/reference/glossary/#term-commit)
atomically when the handler returns — never persisted mid-flight, and never at all
if the handler faults — which keeps each handler's effect on state explicit.

```mermaid
flowchart TD
  ca["call — key a"] --> ia["instance a"]
  cb["call — key b"] --> ib["instance b"]
  ia --> load{"state stored for the key?"}
  load -->|yes| stored["load stored state"]
  load -->|"no — fresh key"| zero["start at the zero value"]
  stored --> run["read store fields; write new values (committed at handler end)"]
  zero --> run
  run --> persist["persisted, per key"]
  ib --> indep["its own independent state"]
```

*A key names a logical instance: calls to the same key share state, different keys
are independent, and a never-seen key starts at the zero value.*

Text equivalent: a call addresses an agent by key, and the runtime selects the
instance for that key (a Durable Object on the `workers` target, an entry in the
`StateRegistry` on `bundle`). Loading returns the stored state, or — for a key
never seen before — the zero value. The handler reads its `store` fields, writes
new values with `:=`, and the runtime commits them for that key when the handler
returns. Different keys (`a`, `b`) are wholly independent instances.

Because an instance's whole state is persisted as one stored value (about 2 MB at
most, read and rewritten by every handler call), the key is also your scaling
lever: spread a growing collection across many keys rather than into one agent. See
[the size and cost of an agent's state](/book/reference/agents/#size-and-cost-of-an-agents-state).

## Why state must be zeroable

Here is the rule that shapes everything: **every `store` field must have a zero
value** (or an explicit initialiser). `Int` is `0`, `Bool` is `false`, `String`
is `""`, `Option[T]` is `None`, and a record is zeroable when all its fields are.

The reason is *fresh-state initialisation*. When you address a key that has never
been seen before, there is no stored state to load — and there is no constructor
you were required to call first. The agent must come into existence with a
well-defined state anyway. Zeroability guarantees that a never-seen key has an
unambiguous starting value, computed by the compiler and baked into the runtime.

This is why a field like `Cell[Int where Positive]` with no initialiser is
rejected: `Positive` excludes `0`, so there is no honest starting value. (Give it
an explicit `= 1` when you have a sensible default.)

In TypeScript, a class can simply assert a field will be set and read it before it
is — `undefined` then flows through as a number:

```typescript
class Gauge {
  level!: number; // "trust me, it's set" — but a fresh Gauge has none
}

const g = new Gauge();
const next = g.level + 1; // compiles; `level` is undefined → NaN
```

In Bynk, every `store` field must have a zero value (or an initialiser), so the
type with no honest zero does not build:

```bynk,fail
{{#include ../../../diagnostics/agents_non_zeroable.bynk}}
```

and the compiler says so — verbatim, captured from `bynk`:

```text
{{#include ../../../diagnostics/agents_non_zeroable.txt}}
```

The fix is to give the field a starting value
([`bynk.agents.non_zeroable_state_field`](/book/troubleshooting/agents-non-zeroable-state-field/)).

## Why "not set yet" is `Option`, not a special case

The temptation, when a field has no natural zero, is to invent an "uninitialised"
sentinel. Bynk refuses that. Instead, "not set yet" is expressed honestly with
`Option`: a `store reading: Cell[Option[Int]]` is zeroable because its zero is
`None`, and `None` *means* "never set". The absence is in the type, where the
rest of the code is forced to handle it — exactly the [errors-as-values
discipline](/book/guides/type-system/philosophy/) applied to state.

So the zeroability rule is not a limitation to work around; it pushes you toward
modelling "absent" precisely, and it is what makes fresh keys safe.

## See also

- Tutorial: [Add a stateful agent](/book/tutorials/05-stateful-agent/).
- How-to: [Build a stateful agent](/book/guides/agents-and-state/stateful-agent/).
- Reference: [agents](/book/reference/agents/).
