---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: Function calls, variant and constructor calls, `unreachable`/`todo`, and the plain method call now lower to typed TypeScript nodes
---
feat(emit): function calls and the plain method call lower to nodes (1A.6)
