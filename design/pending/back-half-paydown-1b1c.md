---
level: patch
probes: ts_writes, ts_any
changelog: Internal restructuring of the code generator; events, the envelope rule and subscriber failure isolation are decided once, and the Bundle root's event dispatch is laid out by the printer
---
refactor(emit): events, the envelope and failure isolation get one home (1B.1c)

Closes #3928. Part of #3676.

`EventSubscriberShape::wants_envelope()` is the one envelope rule; the Bundle dispatch, the Workers `on event` wrapper, the test harness's `svc.event(e)` helper and the callee's synthetic parameter read it. `ComposePlan` records each subscription and its envelope decision. `compose::events::isolated_delivery` builds ADR 0284's failure isolation once, for the Bundle dispatch and the fan-out Durable Object. The Bundle `__eventsDispatch` closure is built from nodes, so five Bundle `compose.ts` goldens are re-laid out by the printer (token-identical). The Bundle root takes its runtime import from the finished tree. An `xtask` test guards that the isolation code appears once in production source.
