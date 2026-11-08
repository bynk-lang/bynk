# 0746 — A WebSocket `on message`/`on close` `by` names the same actors as `on open`

- **Status:** Accepted (v0.512.8)

**Context.** On Workers the principal is verified at the edge during `on open` and carried in the socket attachment; `on message`/`on close` read it from there. A lifecycle handler whose `by` was a different kind (single actor vs. actor sum) emitted TypeScript that either failed `tsc` (unbound `who`) or silently lost the identity (#3046).

**Decision.** `bynk check` requires `on message` and `on close` to name the same set of actors as `on open` (`bynk.ws.lifecycle_by_mismatch`). Binder names may differ and a sum's member order is irrelevant. The rule is in the checker, so it holds on every target, rather than being a Workers emit limitation. Verifying per frame was rejected: it would re-run bearer verification on a hibernated socket that has no token to re-read.

**Consequences.** Programs that gave a lifecycle handler a different `by` than `on open` are now rejected with a note. Per-handler authorisation of different actors on one connection is not expressible.
