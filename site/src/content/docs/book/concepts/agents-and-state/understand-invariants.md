---
title: "Understand: invariants as contracts, tests as behaviour"
---
Validation in Bynk has two complementary shapes, and the difference is the
universal/existential split:

- **Tests describe behaviour** — *there exists a case where this works*. A test
  stands up the agent with controlled inputs and asserts about one run.
- **Invariants describe contracts** — *for all reachable states, this holds*. An
  invariant is a claim the runtime enforces at each handler's commit point.

A reader of an agent sees **examples** of its behaviour (via tests) and
**claims** about its behaviour (via invariants), and the architecture binds both
to the same handler-and-state machinery.

## Why invariants belong on the agent

An agent already owns a piece of state and is the only thing that can change it
(the single-owner rule). An invariant is the direct language-level expression of
what domain-driven design calls an aggregate's *invariants* — the consistency
rules that must always hold for that aggregate. Putting them on the agent means:

- they are **visible at the contract boundary**, not buried in handler bodies;
- they **compose with the failure model** — a violation is a fault, not a typed
  outcome;
- they **reduce test burden** — a property guaranteed by an invariant does not
  need a test case verifying it.

## A worked contrast

```bynk,fragment
agent Inventory {
  key sku: Sku

  store available: Cell[Int]

  invariant available_non_negative:
    available >= 0

  on call reserve(qty: Quantity) -> Effect[Result[(), ReserveError]] {
    if (available < qty) {
      Err(InsufficientStock)
    } else {
      let cur = available
      available := cur - qty
      Ok(())
    }
  }
}
```

The handler's guard makes `available >= 0` true *by construction*. The invariant
captures that intent **once**, at the boundary — so a future refactor that
reorders the guard, or a new handler that forgets it, fails at the commit
boundary rather than silently persisting a negative balance. You do not write a
test for "available never goes negative"; the invariant *is* that guarantee.

## What an invariant may read of a collection

An invariant is checked at every commit, so whatever it reads is read at every commit.
A collection is the one thing it may not scan: asking a `Map` or `Set` whether it
`contains` something, or getting an entry, would read the collection each time. What it
may ask is `size()`, which the commit already knows, so "an order has at most 100 lines"
is `lines.size() <= 100` and costs nothing. Anything beyond that, and any read at all of a
`Log` or a `Cache`, is refused with a message that points you to the handler that writes
the collection, where the one entry in hand is known. The
[reference](/reference/agent-invariants/#collection-size) has the rules.

## What invariants are not

- They are **not** cross-agent. A property like "the sum of reservations across
  all `Inventory` agents equals the original stock" is eventually-consistent and
  belongs to a saga or external monitoring — see
  [the agent model](/book/concepts/agents-and-state/the-agent-model/).
- They are **not** transactional rollback. A fault means the *offending commit*
  is never written, not that the whole handler is undone (effects already sent
  stand). See [Agent invariants](/reference/agent-invariants/).
- They are **not** a typed outcome. A violation is a fault the caller cannot
  pattern-match — it aborts, the same way any other fault does.
