# 0823 — `do` accepts any `Effect[T]` except `Result`

- **Status:** Accepted (v0.571)

**Context.** ADR 0170 B limited `do e` to `Effect[()]`, borrowing the gate from `~>` (ADR 0106 D2). For `~>` the gate has a second job, since a send is never awaited. `do` awaits, so a fault still propagates; the gate only guards against dropping an `Err`, a `Result` problem rather than a valued-reply one. Every `Effect[Int]` write or `Effect[Id]` insert paid the `let _ <- e` ceremony ADR 0170 meant to remove. Proposal #3436, accepted.

**Decision.** `do e` accepts any `Effect[T]` whose payload is not a `Result`. A `Result` payload, as the checker sees it (including through a generic callee instantiated with one; Bynk has no type alias that can denote a `Result`), is refused with `bynk.effect.do_discards_result`, whose quick-fix `do e` to `let _ <- e` is offered but is not machine-applicable. `bynk.effect.do_requires_unit` is retired from the registry and `bynk explain`. `let _ <- e` stays legal for a `Result`. An opaque `T` in a generic body is accepted; `Option` and `HttpResult` are not excluded. A fault in the discarded effect still rejects the handler. This amends ADR 0170 decision B only; A, C, D and E stand, and ADR 0106 is untouched.

**Consequences.** No emitter change: `do` still lowers to `await e;`. Only programs that previously failed to compile change status. The diagnostic code differs for a `Result` operand, and tooling that matched `do_requires_unit` must update. A marker meaning "must not be discarded" could later replace the hard-coded `Result` test without changing what `do` accepts for `Result`.
