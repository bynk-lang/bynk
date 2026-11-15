---
level: patch
changelog: `bynk/architectureModel` lists each event's emitters and subscribers and each handler's capability use (`given`, with the suites that `stub` it)
---

## ADR: architecture-model-events-uses
title: `bynk/architectureModel` carries event emitters and subscribers and each handler's capability use
summary: The architecture model extends one request with additive `events` and per-node `uses` sections, read from the checker's resolved view

**Context.** Event subscriptions were computed only to be validated, and a handler's resolved `given` was read only for the sequence diagram. A client asking "who emits this event?" or "what can this context touch?" could only search text, which breaks on aliases and `consumes` qualification (#3583).

**Decision.** Extend `bynk/architectureModel` rather than add a request (A): both answers are about project structure, and one request is one project walk. Include `stub`, as `stubbedIn` on a capability use (B). Make `uses` per handler with spans, leaving the per-context rollup to the client (C). The checker computes the facts in `bynk_check::architecture_facts` and retains them on the analysis: an event's owner is found as `discover_event_subscribers` finds it, an emitter is a call the checker resolved to `Events.emit`, and a use is read from the handler's `given` after service defaults are injected, resolved through the unit's `consumes` aliases and flattened selections. Delivery kind is `all`, `pattern`, `schema` or `patternSchema`, from the subscription's header.

**Consequences.** The change is additive on the wire; old clients ignore the new fields. Provider operations are listed as members (`kind: "provider"`) because they can emit and carry their own `given`. Spans only, no source text.
