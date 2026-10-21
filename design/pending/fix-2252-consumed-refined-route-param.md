---
level: patch
changelog: An HTTP route whose path parameter is a refined or opaque type from a consumed context no longer faults with a `500` on every request; the entry validates it with the context's own codec for that type, so a bad value gets the usual `400` `RefinementViolation` (#2252)
probes: ts_writes
---
