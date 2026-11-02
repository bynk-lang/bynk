---
level: patch
changelog: "A handler parameter named like its service no longer shadows the service in the generated `__makeSurface` method; the surface renames the parameter to `__id_<name>`"
probes: ts_writes
---
