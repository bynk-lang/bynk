---
level: patch
changelog: "The emitter lowers a built-in method call by its typed `KernelOp`, not by the method's spelling, as the second slice of the typed built-in dispatch (#3164)"
probes: ts_writes
---
