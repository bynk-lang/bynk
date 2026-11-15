---
level: minor
changelog: Naming a type, capability or service that the file's own `consumes` does not bring in is now an error (`bynk.consumes.type_not_selected`, `bynk.consumes.capability_not_selected`, `bynk.consumes.unit_not_consumed`), with the same fixes (#3536)
probes: diagnostic_coverage
breaking: `consumes` is per file, for every unit and for suites: a file that names a type (or a variant), a capability or a service that its own `consumes` does not bring in now fails `bynk check` with `bynk.consumes.type_not_selected`, `bynk.consumes.capability_not_selected` or `bynk.consumes.unit_not_consumed` (these were warnings since v0.614.0). Run `bynk check --fix=bynk.consumes.type_not_selected,bynk.consumes.capability_not_selected,bynk.consumes.unit_not_consumed` to add the clauses to each file, on this version or on v0.614.x, where the fixes are attached.
---
