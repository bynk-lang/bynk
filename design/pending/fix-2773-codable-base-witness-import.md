---
level: patch
changelog: "A test module that calls a `Codable`-bounded generic (or generic static) with a base type such as `Int` now imports the `JsonValue`/`BoundaryError`/`Result` names the inline witness codec uses, instead of failing `tsc` with `Cannot find name '__JsonValue'`"
---
