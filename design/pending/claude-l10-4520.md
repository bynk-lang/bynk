---
level: patch
changelog: A comparison that re-tests a read an enclosing `if` or `match` arm already narrowed to a literal (`if a == 7 { if a == 8 { … } }`) compiles, instead of stopping the build with `bynk.codegen.tsc_failed` (TS2367)
---
