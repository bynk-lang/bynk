---
title: Agent invariants
---
An **invariant** is a property an agent guarantees of *every* committed state. It
is a universally-quantified claim — *for all reachable states, this holds* — and
the runtime enforces it at each commit boundary. Invariants are the contract half
of validation; tests are the behaviour half (see
[Invariants as contracts](/book/guides/agents-and-state/understand-invariants/)).

## Declaration

Invariants form a phase **between the `store` fields and the handlers**:

```bynk,fragment
type OrderStatus = enum { Pending, Placed, Paid }

agent Order {
  key id: OrderId

  store status:     Cell[OrderStatus] = Pending
  store user:       Cell[Option[UserId]]
  store cart:       Cell[Option[Cart]]
  store paymentRef: Cell[Option[AuthId]]

  invariant placed_has_user_and_cart:
    status == Placed implies (user.isSome() && cart.isSome())
  invariant paid_has_payment_ref:
    status == Paid implies paymentRef.isSome()

  on call place(u: UserId, c: Cart) -> Effect[()] {
    status := Placed
    user   := Some(u)
    cart   := Some(c)
    ()
  }
}
```

Each invariant is `invariant <name>: <predicate>`. The predicate references the
agent's **store fields by bare name** — `status`, `paymentRef` — and is a claim
about the *proposed* committed state (the values the handler's writes are about to
persist), not the currently-persisted one.

## The predicate surface

A predicate is an ordinary **pure, `Bool`-typed** expression over the state
fields, with two additions:

- **`implies`** — logical implication. `P implies Q` reads as P → Q and is
  equivalent to `!P || Q`, but directional and prose-readable. It is the
  lowest-precedence operator (below `||`).
- **`is`** — pattern-matching as a `Bool` expression (`order is Placed(o)`),
  with optional bindings that remain in scope across the predicate.

Predicates may call **pure value methods** (`Option.isSome`/`isNone`, sum
`is`-checks). They may **not**:

| Not allowed | Diagnostic |
|---|---|
| A non-`Bool` predicate | `bynk.invariant.not_bool` |
| Effects, capabilities, or test-only constructs | `bynk.invariant.impure_predicate` |
| Referencing another agent | `bynk.invariant.cross_agent_reference` |
| Two invariants with the same name | `bynk.invariant.duplicate_name` |
| An `invariant` after a handler | `bynk.parse.invariant_after_handler` |

Invariants are **per-agent**: they constrain one agent's reachable states. A
property that genuinely spans agents is eventually-consistent — express it with a
saga or a scenario, not an invariant.

## Step invariants — `transition` {#step-invariants}

An `invariant` constrains a single committed **state**; a `transition` constrains
the **move** between two committed states — the *step*. It sits beside the
invariants, in the same phase between the store fields and the handlers, and reads
over two contextual bindings: **`old`** (the last committed state) and **`new`**
(the state this commit would persist):

```bynk,fragment
type OrderStatus = enum { Pending, Placed, Paid }

agent Order {
  key id: OrderId

  store status:     Cell[OrderStatus] = Pending
  store paymentRef: Cell[Option[AuthId]]

  // snapshot: a committed state is internally consistent
  invariant paid_has_payment_ref:
    status == Paid implies paymentRef.isSome()

  // step: a paid order can never become unpaid
  transition paid_is_terminal:
    old.status is Paid implies new.status is Paid

  on call pay(ref: AuthId) -> Effect[()] {
    status     := Paid
    paymentRef := Some(ref)
    ()
  }
}
```

`old` and `new` are each the agent's state record, so `old.status` / `new.status`
read a field like any record access. They are **contextual** — special only inside
a `transition` predicate — so a value named `old` or `new` elsewhere still parses.
The predicate is the same surface as an invariant (`implies`, `is`, operators, pure
methods; pure `Bool`), with the same restrictions:

| Not allowed | Diagnostic |
|---|---|
| A non-`Bool` predicate | `bynk.transition.not_bool` |
| Effects, capabilities, or test-only constructs | `bynk.transition.impure_predicate` |
| Referencing another agent | `bynk.transition.cross_agent_reference` |
| Two transitions with the same name | `bynk.transition.duplicate_name` |
| A predicate mentioning neither `old` nor `new` | `bynk.transition.no_step_reference` |
| A `transition` after a handler | `bynk.parse.transition_after_handler` |

A transition that mentions neither `old` nor `new` is a snapshot claim in
disguise — write it as an `invariant`.

**Ordered transitions need ordered types.** `old.status is Paid implies
new.status is Paid` uses `is`/`implies`, which need no ordering. An *ordered* step
like `new.balance >= old.balance` needs `>=`, available on numeric and temporal
fields but **not on enums** (enums are unordered today) — an ordered-status
transition is a [named follow-on](/book/about/versioning-and-roadmap/).

Transitions are checked at the **commit boundary**, alongside the snapshot
invariants — see below.

## When they fire, and what a violation looks like

Invariants and transitions are both **runtime-checked at the commit boundary**. A
handler runs to completion and stages the state its `:=` writes produce; the
runtime evaluates each invariant against that value — and each transition against
the `(old, new)` pair — *before* it is persisted. If any fails:

- the commit **faults** with an `InvariantViolation` — the offending state is
  never written;
- the failure is a **fault, not an outcome** — the handler's `Result` is never
  produced, consistent with Bynk's failure model.

A transition needs a prior committed state to compare against, so it is checked
from the **second commit onward**: the **genesis commit** (an agent's first) has
no `old` and is skipped — the snapshot invariants still constrain it. Because both
checks live at the commit boundary, they fire at *every* test tier for free.

Intermediate states *within* a handler are not constrained — a handler may
briefly hold an inconsistent state while transitioning, as long as the committed
state satisfies every invariant (the same deferral transactional databases use
for constraints).

> **"Revert" means non-persistence, not rollback.** A fault guarantees only that
> the *staged state is never written* — the handler's `store` writes commit all-or-
> nothing at the boundary. It does **not** undo effects the handler already
> performed (an awaited `<-` call) before it faulted. The handler is not
> transactional. (ADR 0107.)

### Observability limit (MVP)

In v0.80, a violation surfaces to a programmatic caller as a bare **500-class
fault** — observationally identical to any other internal fault. The compiler
*does* `console.error` the agent type and invariant name (never the key value, to
keep domain identifiers out of logs) at the commit site, so a refusal is
distinguishable from a crash **in the logs**. Making the refusal
caller-distinguishable is a [named follow-on](/book/about/versioning-and-roadmap/)
(a general typed-agent-fault channel), as is the compile-time
provable-violation pass.

## The other half: history properties

`invariant` and `transition` are the **commit-boundary** half of the invariant↔test
duality — carried by the code, checked when a handler commits, inherited by every
`case` at every tier for free. The **runner-adversarial** half is a
[history property](/book/reference/testing/#history-properties): `for all run:
History[Agent]` drives a generated sequence of the agent's handlers and judges the
whole reached run. The two halves see the *same* reached states — a `transition`
sees one `old`→`new` step at the boundary; a history property sees the ordered list
of those steps (`s.old` / `s.new` / `s.accepted`) in the runner — so a claim you can
express per-step you carry as a `transition`, and a claim that spans steps ("no
accepted spend without a prior accepted top-up") you assert as a history property.
