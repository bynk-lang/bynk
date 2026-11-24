# 0910 — `svc.outcome(…)` returns a `CallOutcome`, so a caller can handle a failed call

- **Status:** Accepted (v0.656)

**Context.** Every failure of a cross-context call throws in the caller:
`ContractMismatch`, a marked `CalleeFaulted`, `Transport`, an unparseable
response. Faults are uncatchable (ADR 0444), so the calling handler faults, and
the `let … else` compensation in the consume-services guide never runs when the
call *fails*, only when it answers `Err`. Outbound HTTP returns
`Result[Response, FetchError]`, so an external call was safer to handle than a
call to one's own context. Only the compiled call knows why it failed, so only
the language can make that a value without opening a general way to catch faults.

**Decision.** A. The form is a member of the service reference,
`<context>.<service>.outcome(args)`, typed `Effect[CallOutcome[R]]` for the
handler's `R`; it is not a method of `Effect`, so no method is valid on some
effects and not others, and the service's declared signature does not change.
A callee-declared result type and a catch-anything `.result()` are rejected. A
service named `outcome` keeps its plain call. B. `CallOutcome[R]` is
`Handled(result: R) | Rejected(reason: Refusal) | Faulted | Unreachable(status: Option[Int])`,
grouped by whether the handler ran: `Rejected` no, `Faulted` yes and committed
nothing, `Unreachable` unknown. `Refusal` maps one to one onto the wire checks:
`413` `TooLarge`, `401` `NotAuthenticated`, `400 UnsupportedWireVersion`
`UnsupportedWire`, `404` `NoSuchService`, `403` `NotAdmitted` (the runtime
admission gate of `by` refinements, #4182, answers it), `409` `StaleContract`,
any other `400` `Undecodable`. A marked `500` is `Faulted`; any other status is
`Unreachable(Some(status))` (so a callee's `503` for a busy agent, #3932, is
`Unreachable`), and a `fetch` that throws is `Unreachable(None)`. An unreadable
`200` stays a fault: contract hashes make it a toolchain bug. C. `o.handled()` is
`Result[R, CallFailure]`, an ordinary method, so `?` and `embeds` propagate it;
`CallFailure` is the three non-handled variants as an ordinary sum. The method is
written in `bynk` as `fn CallOutcome.handled(self)`. D. No execution context is
needed: `.outcome` changes only how a failure is represented, so it is legal
wherever a cross-context call is, agent handlers included, and `.within(d)`
composes with it. E. A failure turned into a value is still logged once, with
today's `ContractMismatch` and `CalleeFaulted` lines, and one `Call<Variant>`
line for the rest. F. The types live in `bynk` (a unit consumes them, with a
"not in scope" note, `bynk.call.outcome_not_in_scope`) and a context cannot
construct `CallOutcome` or `CallFailure` (`bynk.call.outcome_construction`); only
`.outcome(…)` and a test stub make them. `Refusal`, a plain enum, is not
guarded. G. On the `bundle` target only `Handled` and `Faulted` occur: an
in-process callee fault is `Faulted`, and there is no wire to refuse or drop the
call. H. A consumed service's `stub` gains `rejected <Refusal>`, `faulted` and
`unreachable`, extending #3892; `returns <value>` means `Handled(value)`. They
are a consumed service's alone (`bynk.stub.outcome_word`) and are not accepted
in `returns each`. A stub throws an error marked `bynkCallStub`, which the
bundle's `__callOutcome` reads back. I. Not now: declared compensation
(`undo`), because "the handler failed" needs defining and an undo can itself
fail; recorded for a later track.

**Consequences.** A handler can undo, reconcile or retry on exactly the
failures that need it, and the guide's compensation recipe covers a call that
fails as well as one that answers `Err`. `Rejected`, `Faulted` and `Unreachable`
appear in both `CallOutcome` and `CallFailure`; patterns resolve against the
scrutinee's type, so the fallback shape (`Handled(R) | Failed(CallFailure)`) was
not needed. Adding three exported types to `bynk` makes a bare `consumes bynk`
clash with a user type of the same name, recorded as breaking.
