# 0891 — An `on call` handler must state its actor

- **Status:** Accepted (v0.642)

**Context.** ADR 0082 gave `on call` a silent default actor, `Caller`, on the
premise that the `Internal` channel is trusted ("the channel itself is the
assertion"), and made HTTP the exception because it has no safe default. Both
reasons have changed. The premise was false once `/_bynk/call/` answered on a
Worker's public address (ADR 0479), and calls are safe now only because they are
signed per edge (ADR 0485). And there is a real choice to make: `Caller`, a
refinement over it, a `Foreign` actor (ADR 0486) or a sum. A silent default means
"any consuming context", a decision nobody wrote down.

**Decision.**

- **A.** A service's `on call` handler with no `by`, on the handler or inherited
  from the service header (ADR 0181), is the error
  `bynk.actor.missing_by_on_call`. Its fix-it, machine-applicable, inserts
  `by Caller` after the return type, and `bynk check --fix=bynk.actor.missing_by_on_call`
  migrates a project in one command.
- **B.** `by` is required where the caller is a party and more than one actor is
  sensible: `http` and `websocket` (already), `call` (this increment) and, in a
  later increment, `email`. Silent defaults stay where the platform is the only
  possible caller: `cron` (`Scheduler`), `queue` (`Producer`) and `Events`.
- **C.** Agents are unaffected: an agent's `on call` operations are reached only
  from inside their own context, so there is no caller to name.
- **D.** This is a breaking change. Bynk is pre-1.0 and the fix is mechanical.

**Consequences.** The default actor for `call` is gone from `default_actor`; the
emitter is unchanged, because `by Caller` emits what the default emitted, so the
generated output of every migrated program is byte-identical. Every fixture,
example and book snippet in the repository was migrated with the fix-it. A
service header `by` still covers every handler without its own.
