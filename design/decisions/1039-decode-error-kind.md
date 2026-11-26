# 1039 — DecodeError.kind is a closed DecodeErrorKind sum

- **Status:** Accepted (v0.777)

**Context.** `DecodeError.kind` was a `String`: a typo (`e.kind == "Malfomed"`) compiled and was never true, and nothing told an author which kinds exist (three for JSON, nine more for YAML).

**Decision.** (A) One closed sum, `DecodeErrorKind`, for every decoder, declared in the first-party `bynk.decode` commons and registered as a prelude type, reached with no `uses` by a unit that names it or reads `.kind`. (B) A comparison of `e.kind` with a string literal is a type error carrying a note, and a machine-applicable fix to the variant when the literal names a kind; a literal that names none gets no fix and the note says so. (C) `"\(e.kind)"` prints the variant's name. (D) The variants resolve by scrutinee type, as `Ordering` and `ValidationRule` do. The decoders build the variant value (`{ tag: "Malformed" }`); `rejectionDetail` and the HTTP `400` bodies still write the name, so the wire is unchanged. A single-file program cannot reach a first-party unit, so it cannot read `e.kind`, as it cannot read `e.rule`.

**Consequences.** A future decoder kind is a breaking change to exhaustive matches, which is intended. Existing string comparisons are migrated by the fix.
