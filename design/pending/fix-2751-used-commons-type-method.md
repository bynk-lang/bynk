---
level: patch
changelog: A context calling a used commons' method on a record or sum type no longer emits `Tag.label(t)` against a type-only binding that fails `tsc`; the commons' value is re-exported alongside the rebranded type (#2751)
---
