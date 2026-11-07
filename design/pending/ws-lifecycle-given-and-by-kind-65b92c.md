---
level: patch
probes: emit_diagnostics, diagnostic_coverage
changelog: Workers WebSocket `on message`/`on close` rebuild the agent handler's `given` and events deps, and `bynk check` rejects a lifecycle `by` of a different kind than `on open`'s (`bynk.ws.lifecycle_by_mismatch`)
---

## ADR: ws-lifecycle-by-matches-open
title: A WebSocket `on message`/`on close` `by` names the same actors as `on open`
summary: The connection's principal is verified once at `on open` and carried; lifecycle handlers must name the same single actor or sum members

**Context.** On Workers the principal is verified at the edge during `on open` and carried in the socket attachment; `on message`/`on close` read it from there. A lifecycle handler whose `by` was a different kind (single actor vs. actor sum) emitted TypeScript that either failed `tsc` (unbound `who`) or silently lost the identity (#3046).

**Decision.** `bynk check` requires `on message` and `on close` to name the same set of actors as `on open` (`bynk.ws.lifecycle_by_mismatch`). Binder names may differ and a sum's member order is irrelevant. The rule is in the checker, so it holds on every target, rather than being a Workers emit limitation. Verifying per frame was rejected: it would re-run bearer verification on a hibernated socket that has no token to re-read.

**Consequences.** Programs that gave a lifecycle handler a different `by` than `on open` are now rejected with a note. Per-handler authorisation of different actors on one connection is not expressible.
