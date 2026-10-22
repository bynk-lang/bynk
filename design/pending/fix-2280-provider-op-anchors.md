---
level: patch
changelog: "`bynkc test --coverage` anchors a provider op's header, and the code after the last op, to the right `.bynk` lines"
---

## ADR: provider-op-source-map-anchors
title: Provider op headers anchor to their own line in the emitter's source map
summary: Where a provider op's generated header and the class tail map

**Context.** Agent handlers (#2278) and service handlers (#2280) now anchor their generated headers to their own source line. A provider's ops had the same gap: an op's header mapped to the previous op's last body line, and the class's closing brace and the load-time provider registration inherited the last op's final body line, so an uncalled last op could show a covered line.

**Decision.** `emit_provider` records a source-map checkpoint at each op's start, anchored to the op's span, and one after the last op, anchored to the provider declaration.

**Consequences.** A provider's uncalled op is reported uncovered from its header down, and a called one stays covered, whichever subset of cases runs.
