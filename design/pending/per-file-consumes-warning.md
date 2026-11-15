---
level: minor
changelog: `consumes` is per file (warning stage): naming a type, or a bare variant of a sum, that a unit exports in a file whose own `consumes` does not bring it in is `bynk.consumes.type_not_selected`, for the standard library and for the project's own contexts alike, and `bynk check --fix=bynk.consumes.type_not_selected` adds it to that file's `consumes`, in any fragment of a split unit; an error follows in a later release (#3536)
probes: emit_diagnostics, diagnostic_coverage, explained_codes
---
