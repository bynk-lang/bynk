---
level: patch
changelog: A context calling a used commons' method on a record, sum or alias type no longer emits `Tag.label(t)` against a type-only binding that fails `tsc`; the method is forwarded on a value alongside the rebranded type (#2751)
---
