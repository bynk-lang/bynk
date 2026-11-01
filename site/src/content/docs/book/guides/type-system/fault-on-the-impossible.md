---
title: "Fault on the impossible or the unfinished"
---
**Goal:** mark a case that cannot happen, or code that is not written yet,
without inventing a value to satisfy the type checker.

Errors are values: a failure a caller could handle belongs in a `Result`. Two
built-ins are for the other kind of situation, where continuing would be wrong.

## Mark a case that cannot happen

```bynk,fragment
fn label(s: Status) -> String {
  match s {
    Open => "open",
    Closed => "closed",
    Archived => unreachable("archived orders are filtered upstream"),
  }
}
```

The `unreachable` arm takes the type of the other arms, so there is no
placeholder like `""` to silently mask a broken assumption. If the arm is ever
evaluated, the handler faults with the message. An agent's state is not
committed, and the logged fault names the message and the source location.

## Leave unfinished code in place

```bynk,fragment
fn refund(o: Order) -> Effect[Result[(), RefundError]] {
  todo("refunds — #1234")
}
```

`todo` type-checks as whatever the position needs, so the rest of the program
builds around it. It reports the warning `bynk.fault.todo`; run
`bynkc check --deny-warnings` in CI to refuse a build that still contains one.

## Assert the fault in a test

```bynk,fragment
case "an archived order is never labelled" {
  expect labels.call(Archived) faults
}
```

## Do and don't

- **Do** use `unreachable` for an invariant the program has already enforced
  elsewhere.
- **Don't** use it for input a caller could get wrong: return a `Result` so the
  caller can handle it. There is deliberately no general `panic`.

**See also:** [Reference — Deliberate faults](/book/reference/types/#faults),
[Spec §5.2](/book/spec/static-semantics/#52-well-typedness).
