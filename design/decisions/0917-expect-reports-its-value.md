# 0917 — A failed expect reports the value it tested

- **Status:** Accepted (v0.662)

**Context.** Only a top-level comparison reported expected-vs-actual. A failed `expect r is Ok(_)` said nothing of what `r` was, and `_ => expect false` was just as silent (#3980).

**Decision.** (A) For `expect s is P`, where `s` is a name or a field path, the failure adds `<s> was <value>`; a call or other expression reports nothing, so re-reading it never repeats an effect. (B) The value is shown by the test runtime's value printer (`__bynkShow`), computed only when the expectation fails, in the runtime's tagged form (`{"tag":"Err","error":"…"}`) rather than `Json.encode`'s `kind` form. A `Json.encode` codec closure generated in the test module breaks `tsc` in cross-context suites (a clashing `Region`, an unbound `SqlError`), so none is generated. A subject that is not plain data shows its type (`an Option[Stream[Int]]`). The message cuts a value at 2,000 characters and says so, and the `--format json` case record carries the whole value as `actual` (omitted for a type name or a redacted value). (C) An `expect false` that is a whole `match` arm names the arm and shows the scrutinee when that is a name or field path. (D) A value whose type is, or contains in `Result`/`Option`/collection/generic arguments, `SigningKey`, `VerifyKey` or `TokenClaims` shows `<redacted>`.

**Consequences.** `__bynkExpectFailure` and `__bynkExpect` take an optional label and thunk, so every emitted test module's runtime text changes. A record whose field holds a key is not inspected for redaction (such a record is not `Codable`, and a key cannot reach a test case through a handler boundary). The VS Code explorer reads `actual` from the record.
