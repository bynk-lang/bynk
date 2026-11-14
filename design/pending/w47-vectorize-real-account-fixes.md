---
level: patch
changelog: Vectorize fixes from a real-account check (#3479): `describe` and the `bynk dev` first-call check work against a real index (`IndexInfo.metric` is an `Option`), a query naming no namespace returns every namespace's vectors in the test double, and a `topK` over the limit is `VectorInvalidInput`
probes: emit_abi_shapes
breaking: `IndexInfo.metric` (the result of the experimental `Vectorize.describe`) is now `Option[String]`, because a real index reports no metric; code that reads `info.metric` as a `String` fails to type-check, so match it as `Some(metric)` or `None`
---
