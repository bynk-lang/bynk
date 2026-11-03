# 0716 — A reserved word can be a name wherever the name is a label, never where it binds

- **Status:** Accepted (v0.493)

**Context.** A record field is a JSON key (`reference/wire-format.md`), and a
third-party payload picks its own keys. A GitHub webhook carries `type` on every
account and sender, `from` on a rename and `actor` on events. Every hard keyword
was refused as a field name (`bynk.parse.reserved_keyword`), and the language has
no mapping from a JSON key to another field name, so such a payload could not be
decoded into a typed record at all. #2778 hit the same wall from the other side:
`Sorted.from` (constructors named `from`) shipped as `Sorted.fromList` (ADR 0702).
Emission already copes: JS allows reserved words as property names, record fields
are wire format, and a reserved name in a binding position is already renamed
(`__id_<name>`, #1653).

**Decision.** A keyword is a name wherever the name is a *label*, and never where
it binds or references a variable.

- A field declaration, a construction initialiser, a record or named pattern
  field and a variant payload field admit a keyword **when it is followed by
  `:`**. `kind` stays reserved as a variant payload field, because it is the wire
  tag (`bynk.resolve.reserved_payload_field`).
- A member after `.` (a field access or a method call) and a method declaration
  (`fn Sorted.from(…)`) admit a keyword **when it is on the same line as the
  `.`**, so a half-typed `foo.` above a line starting `let` still stops at the
  `let` in an editor. Leading-dot chains are unaffected.
- A parameter, a `let` or lambda binding and a bare reference refuse a keyword
  as before.

The `:` lookahead is what keeps shorthand refused without a separate rule.
`Account { type, .. }` as a pattern would bind a variable called `type`, and
`Account { type }` as a construction would read one; both are
`bynk.parse.keyword_field_shorthand`. The pattern form carries a `MaybeIncorrect`
fix to `type: type_` (a trailing underscore, since a leading one means "unused");
the construction form carries a note to write `type: <value>` and no fix, as no
variable called `type` can exist to suggest. The parser cannot see scope, so the
fix's binding is the first `type_`, `type_2`, … that no identifier in the file
spells, which is conservative about what is already bound.

Rejected: a raw-identifier escape (`` `type` ``, `r#type`), noisy in exactly the
JSON-shaped records this targets and addable later if bindings need it; a
`from`-only exemption (#2778's third option), which leaves `type` and `self`
broken; admitting keywords as identifiers everywhere, which makes statement
heads ambiguous.

A key-mapping annotation for keys that are not Bynk identifiers at all
(`content-type`, `$ref`, GitHub's `"+1"`) is deferred. When it comes it should
take the shape of #918's envelope sources (`= Header("…")`), not a second syntax.

**Consequences.** `Json.decode`/`Json.encode`, destructuring and the TypeScript
emitter need no change: property names are the field names. `tree-sitter-bynk`
needed none either: a keyword that is not valid in the parser state already lexes
as an `identifier`, so these positions parse; it is more permissive than the
compiler at the binding positions and cannot see the same-line rule, as before.
Semantic tokens, hover and completion read the index, which is keyed by name and
span, so a keyword-named field is a `property` token and a keyword-named method
a `method` token. The vscode-bynk TextMate grammar colours these as keywords until
it is changed. `ADR 0702`'s `Sorted.fromList` could now be spelled `Sorted.from`;
that rename is not part of this change.
