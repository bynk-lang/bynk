---
level: minor
changelog: A first-party capability operation can carry an internal `@onUndecodable(miss|fault)` annotation, refused in user code
probes: diagnostic_coverage, explained_codes, emit_diagnostics
---

No decision record of its own: the annotation's meaning (the failure policy of a typed capability read) is recorded by the ADR that lifts ADR 0696 §5 in the PR that follows. This PR adds only the syntax slot and its refusal outside the standard library.
