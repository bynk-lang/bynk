# 0786 — JsonError is renamed DecodeError, with a deprecated alias

- **Status:** Accepted (v0.536)

**Context.** ADR 0047 made `JsonError` the compiler-known record (`kind`, `path`,
`message`) that `Json.decode` returns. A second decoder, `Yaml.decode[T]` (proposed in
#3215), returns the same record, and a name that says "JSON" would be wrong there. #3214
asked for `DecodeError`; the accepted proposal (#3215) amended its Decision B to rename
first, as its own increment, so both decoders share one type.

**Decision.**

- **The type is `DecodeError`.** It is the same compiler-known record, unchanged in shape:
  `kind`, `path`, `message`, all `String`. Only its name changes, in the checker
  (`Ty::DecodeError`, `TypeRef::DecodeError`), the lexer, the emitted TypeScript
  (`__bynk_runtime/errors.ts` exports `DecodeError`), the standard library's
  `bynk.cloudflare` source, the editor surface and the Book.
- **`JsonError` stays as a deprecated spelling.** The lexer still reserves `JsonError`, so no
  program can declare a type of that name; the parser reads it as the same type and emits the
  warning `bynk.types.deprecated_json_error` (ADR 0117) carrying a machine-applicable
  rewrite to `DecodeError` (ADR 0054). Existing code keeps compiling, and `bynk fmt`, which
  prints from the AST, rewrites the spelling too. This follows the precedent of
  `bynk.list.deprecated_function` (ADR 0116 D6) and ADR 0468's rule that a stable item is
  deprecated in a minor release and removed only in a major one. The alias is removed when
  that policy allows, in its own increment.
- **`DecodeError` becomes a reserved word.** A program that declared its own type named
  `DecodeError` stops compiling; the fix is to rename that type. No such declaration
  exists in the repository's fixtures, examples or standard library.
- **The wire is unchanged.** `DecodeError` is not codable (as `JsonError` was), so it never
  crosses a boundary and no contract hash of a deployed context moves. The emitted
  TypeScript type name changes; a hand-written TypeScript consumer that imported
  `JsonError` from the emitted runtime must import `DecodeError`.
- **`kind` text is unchanged** (`Malformed`, `StructuralMismatch`, `RefinementViolation`), so
  code that branches on it is unaffected. The doc text of `DecodeError` says "decode error",
  not "JSON-decode error".
- This amends ADR 0047, which keeps its history under the old name; it is not rewritten.

**Consequences.** The one decode error type is ready for `Yaml.decode[T]` to return without
a name that misleads. Migration is mechanical: replace `JsonError` with `DecodeError` in type
positions (apply the warning's fix, or run `bynk fmt`), and rename any user type called
`DecodeError`. Editor surfaces (completion, hover, the TextMate grammar) know both spellings,
and the deprecated one is documented as such.
