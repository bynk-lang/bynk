---
level: minor
changelog: A `system` or `integration` case can read a participant's test D1 database with `D1.peek(<participant>.<Capability>)`, a read-only handle whose SQL is checked against the migrations (checker half)
---

## ADR: d1-peek-read-only-test-handle
title: A case reads a participant's test D1 database through a read-only `D1.peek` handle
summary: A case-only built-in reaches behind the system boundary to read, never write, the database the app wrote

**Context.** A `system` case drives the whole deployable app through its real boundaries
(ADR 0153, ADR 0207), so it cannot name a capability: `Orders.all(…)` there is
`bynk.resolve.unknown_name`. A test that must check "this request stored this row" had to do
it through the app's own routes, which is awkward or impossible when the app exposes none.
The harness already builds each participant's `env` with an in-memory D1 database, and the
app's own D1 calls run against that object in the same process as the case, so the instance
exists and the case only lacked a way to reach it. This realises the follow-on ADR 0153 left
open, observing across the `system` wire, for D1, in the spirit of the harness-controlled
clock (ADR 0523): the harness controls something behind the app, and the app is untouched.
Accepted in #3147.

**Decision.**
- **Surface.** `D1.peek(<participant>.<Capability>)` is a case-only built-in. It binds a
  handle (`let db = D1.peek(shop.orders.Orders)`) whose only use is as the receiver of the
  read operations, with the capability's own signatures: `all`, `first`, and
  `select[T]` / `firstAs[T]`. It resolves in an `integration` or `system` case; elsewhere
  it is `bynk.test.peek_outside_case`. It needs no `consumes bynk.cloudflare`, being no use
  of the `D1` capability.
- **Misuse is refused**, each with its own code: outside such a case
  (`bynk.test.peek_outside_case`), a capability that is not derived from `D1`
  (`bynk.test.peek_not_a_database`), a participant or capability the suite does not have
  (`bynk.test.peek_unknown_capability`).
- **Read-only by construction.** `run` and `batch` are refused on the handle, and a
  statement that would write is refused (`bynk.test.peek_write`): at check time, from the
  prepared statement's `readonly` flag, for a literal or `const` SQL argument (SQL is
  literal-only for `all`, `first`, `select` and `firstAs`, so no statement escapes this),
  and again at run time by the harness.
- **Schema-checked.** A peeked statement is prepared against its own participant's
  migrations, as the app's own D1 statements are.
- **One object.** The handle reads the same database object the app's `env` holds, so it
  sees exactly what the app wrote in the case.
- **`--against`.** A case that uses `D1.peek` is reported as skipped, with the reason
  "needs the local test database", not failed and not silently passed: a deployed target has
  no in-process database, and reaching a deployed D1 would make a test run touch a real one.
- **Seeding is out of scope**, tracked in #3563. The handle stays read-only so a case cannot
  write state the app's invariants never saw.

**Consequences.** A `system` case can assert on what the app stored without the app growing a
route for it. The `D1` value types (`SqlValue`, `SqlError`, `SqlRun`, `SqlStatement`) are in
a `system` case's scope wherever a participant has a D1 capability, so a case can write the
parameters of a statement. The handle is not a first-class value: it has no type a program
can write, and cannot be passed or returned. Editor support follows in a later slice.
