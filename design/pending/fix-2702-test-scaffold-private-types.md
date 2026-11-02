---
level: patch
changelog: A test scaffold no longer resolves a type name to a used commons' `private` type, so `Json.decode[T]` and stub signatures bind the visible `T` (#2702)
---
