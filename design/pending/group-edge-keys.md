---
level: patch
changelog: A `[deploy.workers.<key>]` group's calls are signed and verified under keys named for its member contexts, so a group can call across an edge and its members can call each other once deployed or run under `bynk dev` (#3562, G4-4)
---

The runtime signs a cross-context call as the calling *context* and verifies the
callee's `BYNK_CALL_FROM_<that context>`, but `deploy` and `dev` named each edge's keys
after the Worker's directory. For a group (`front`, serving `shop.a1` and `shop.a2`)
that put `BYNK_CALL_FROM_FRONT` on the Worker it calls, which nothing verifies, and
set no key at all for a call between two members (which G3-2 now makes in process, but
still signs).

An edge is still one secret per pair of Workers, now named per member context: the
callee Worker holds a `BYNK_CALL_FROM_` key for each member of the calling Worker, the
caller a `BYNK_CALL_TO_` key for each member of the called one, and a group holds both
for each of its own members (an edge to itself, which is a key edge though no
deploy-order edge). An end holds the edge's key only if it holds every name, so a
partly keyed Worker is keyed afresh. A Worker that is one context is named as before.
ADR 0874 gains D7's note that the keys are per member and shared among a Worker's
members, who share one `env` anyway.
