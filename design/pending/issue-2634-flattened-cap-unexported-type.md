---
level: patch
changelog: A flattened capability (`consumes U { Cap }`) whose operation names a type `U` declares but does not export now resolves that type, as the qualified form does, instead of silently typing the call as `()`; calling an operation whose signature names a type the consumer declares with a different shape is refused with `bynk.boundary.structural_mismatch`
---
