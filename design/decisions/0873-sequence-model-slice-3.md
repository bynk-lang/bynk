# 0873 — The sequence model is complete to a safety cap, shows `?` as a break, and names its trigger, agent keys and callees

- **Status:** Accepted (v0.633.18)

**Context.** The model-diagrams track (`design/tracks/model-diagrams.md` §6.3)
asks the server for the facts behind a better sequence diagram. ADR 0858 says
the server's diagram models are complete and the client decides how much to
show, but `bynk-ide::sequence` still cut nesting at depth 2, a presentation
choice made for Mermaid. The walk also ignored `?`, so a handler's early
return (`let raw = stored?`, a 404 under `HttpResult`) was invisible. A
handler with no principal had nothing to say what starts it. An agent lifeline
did not show which instance was called. A boundary call stopped at the
boundary (Decision C) with no way for a client to reach the callee. The walk
is syntactic: it sees the AST and the unit's cross-context and agent tables,
not types.

**Decision.**

- **Complete to a safety cap.** `MAX_BLOCK_DEPTH` becomes 16, a bound for
  pathological input rather than a presentation choice, and each `AltBlock`
  carries its `depth` (0 at the top level). Only nesting past the cap
  collapses. This implements ADR 0858 for the sequence model.
- **`?` is a `break`.** A `?` at the top of a statement's value, or a tail, is
  an `AltKind::Break` block. Its one branch is labelled by the operand
  (`stored?`), and its outcome is an early return: a `Return` to the principal
  inside the block, or, with no principal, the branch's `reply`. The walk
  carries on after it, as the code does.
  - This settles the track's Q5 (which `?` sites to show) as *every one the
    walk sees*. A `?` inside a lambda returns from the lambda, and lambdas are
    not walked, so each `?` shown does reach the handler's result. Without
    types the label says "early return", not which error.
- **`trigger`.** The model names what starts the handler: `Request`, `Call`,
  `Schedule`, `Event`, `Message`, `Connection` or `Rejected`, with the
  handler's request descriptor as its label. `Message` covers both queue and
  WebSocket `on message`, because the model does not see the service's
  protocol.
- **`Participant.key`.** An agent lifeline carries its key expression as
  called (`client`), or `…` when one handler keys the same agent differently.
- **`Message.callee`.** A `Call` or `Send` to an agent or a consumed context
  names the handler it reaches: `{unit, owner, method}`, read from the same
  call shapes the classifier uses. The LSP resolves a `location` for it among
  the callee unit's files in the committed round, `null` when not found.
  Decision C stands: the callee's body is never walked into the caller's
  model, and a client opens the callee's own diagram instead (a UML `ref`).

**Consequences.**

- All wire changes are additive (`trigger`, `key`, `callee`, `depth`, and the
  `Break` kind), so older clients ignore them.
- A handler with deep nesting now sends every level. The cap bounds the
  payload, and the client folds by depth.
- The per-message type and effect details the track lists in §6.3 are **not**
  part of this: they need the checker's types, which this syntactic walk does
  not have, and are left for a later slice.
- The `bynk-lsp` handler-kind snapshots gain the new fields.
