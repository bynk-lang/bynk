---
level: patch
changelog: Aliased types in a service signature resolve for a callee's own callers (the contract now hashes the renamed type's shape); `consumes_item` node in tree-sitter; aliased services run under `workerd`
---

## ADR: consumes-alias-transitive-callers
title: A context's service signature that names a renamed type is read by its callers under the alias
summary: The callee's own alias keys its contract view, so a third context calls it and the hashes agree

**Context.** #2889 let a context rename an exported type (`consumes lib { Code as
LibCode }`). A third context that consumed that context and called a service
taking `LibCode` was refused (`bynk.boundary.structural_mismatch`, "expects `()`"):
the callee's view of its own types, which a caller resolves the service's
signature through, had no entry for the alias. The same gap meant the callee's
*own* contract stated the parameter as an unresolved name, so a change to the
renamed type's variants did not change the service's hash.

**Decision.** `contract_types_for` files each type a unit renames under its alias
as well, so a service signature naming it resolves (to the type under the alias's
name) for the unit's own contract and for every caller. The contract therefore
hashes the type's shape, and a caller derives the same hash. The TS a caller emits
for such a type (casts, codecs) names the module's type by the name its unit
declares: `CrossContextInfo.callee_type_aliases` carries the callee's aliases.

**Consequences.** Contract hashes of services whose signature names an aliased type
change once (the shape is now part of it). Renaming such an alias changes the
service's contract hash for its callers. A caller that does not alias the type can
pass and discard such a value but cannot name it (its type is the callee's alias).
