---
level: patch
changelog: String interpolation holes accept only `String`, `Int`, `Float`, `Bool`, `Decimal` and refinements of them, as ADR 0075 and 0698 specify — `Bytes`, `Duration` and `Instant` holes are now `bynk.types.interpolation_non_scalar` errors (use `toHex()`/`toBase64()`, `toString()`, `toIso()`) (#2799)
---
