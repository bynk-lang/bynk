---
level: patch
probes: ts_writes, lowering_text_bridge
changelog: Value binary operators (arithmetic, comparison, equality, and the `Decimal`, `Int` division, structural-equality and `Ord` helper forms) now lower to typed TypeScript nodes; the printer brackets them by a precedence table
---
