---
level: patch
probes: emit_diagnostics, diagnostic_coverage
changelog: "A Workers WebSocket `on open` keyed by a context `const` now builds, a literal key with a route parameter now reaches the instance the body addresses instead of one keyed by the parameter, and any other key is a compile error on Workers (`bynk.ws.open_transfer_key`)"
---

## ADR: ws-open-transfer-key
title: A Workers WebSocket `on open` transfer is keyed by a route parameter, a constant or a literal
summary: What the edge can evaluate to route an upgrade before the body runs, and why other keys are rejected

**Context.** On Workers, `compose.ts` routes a WebSocket upgrade to the Durable Object of the agent the `on open` transfers its connection to, and it does so before the body runs (ADR 0132). The emitter assumed the transfer's key was a route parameter. A context `const` was emitted as a bare name that `compose.ts` does not bind, so `tsc` failed; constants are functions in `handlers.ts`. Any other key, a literal included, was replaced by the first route parameter, so `Room("lobby")` in `on open (user: String)` routed each upgrade to `Room(user)` and a broadcast to `Room("lobby")` reached no one (#2959). `bynk check` accepted both.

**Decision.** On the Workers target the key must be a parameter of `on open`, a constant of the context, or a string, integer or boolean literal. The edge evaluates a parameter as its validated value, a constant as `handlers.<Name>()`, and a literal as itself, so it addresses the same instance the body does. A name a top-level `let` of the body binds is a local, not the parameter or constant it shadows. Any other key is a compile error, `bynk.ws.open_transfer_key`, raised from a Workers-only project phase beside `bynk.ws.open_transfer_cross_file` (ADR 0577). The emitter no longer falls back to a route parameter: an unroutable key it is handed anyway answers `500`. The bundle target is unaffected, since there the open is an ordinary surface method that evaluates any key.

**Consequences.** A context `const` or a literal can key the hosting agent, and the silent misroute is gone. A key computed in the body, such as one derived from the actor's identity, is rejected on Workers; routing it would mean evaluating the body's prefix at the edge, a larger design than this fix. A `const` from a `uses`d commons is not accepted yet.
