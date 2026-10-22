---
level: patch
changelog: "`bynkc test --coverage` anchors a service handler's header, and the code after the last handler, to the right `.bynk` lines"
---

## ADR: service-handler-source-map-anchors
title: Service handler headers anchor to their own line in the emitter's source map
summary: Where a service handler's generated header, prologue and the code after the last handler map

**Context.** #2278 anchored agent handler headers to their own `on call` line. The service path had the same gap: a handler's header and prologue mapped to the service declaration (or the previous body), and the code after the last handler inherited that handler's final body line, so an uncalled last handler could show a covered line (#2280).

**Decision.** `emit_service` records a source-map checkpoint at each handler's start, anchored to the handler's span, and one after the last handler anchored to the service declaration.

**Consequences.** An uncalled service handler's own lines are reported uncovered, and a called one stays covered, whichever subset of cases runs.
