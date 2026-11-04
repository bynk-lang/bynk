# 0731 — A context's service signature that names a renamed type is read by its callers under the alias

- **Status:** Accepted (v0.505.1)

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
service's contract hash for its callers. A caller that does not itself alias the type
reaches it under the callee's alias, so it can pass such a value on but not match
on it or return it as its own `Code` (a known gap, to be closed by resolving the
callee's alias to the declaring type for the caller).
