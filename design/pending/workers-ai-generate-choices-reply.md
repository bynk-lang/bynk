---
level: patch
changelog: `Ai.generate` reads the reply text from `choices[0].message.content` when a model sends no `response`, as granite does; `response` still wins when both are there
probes: ts_writes
---
