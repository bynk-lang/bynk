---
level: patch
changelog: Vectorize fixes from a real-account check (#3479): `describe` and the `bynk dev` first-call check work against a real index (`IndexInfo.metric` is an `Option`), a query naming no namespace returns every namespace's vectors in the test double, and a `topK` over the limit is `VectorInvalidInput`
probes: emit_abi_shapes
---
