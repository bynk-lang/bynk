---
level: minor
changelog: A cookie is written from the record that reads it with `Prefs.theme.cookie(Dark)` and `Prefs.theme.expire()`, named by the field's wire name and valued as the decoder reads it
---

## ADR: typed-cookie-writes
title: A typed cookie write from the cookie record's field
summary: `Record.field.cookie(v)` and `Record.field.expire()` are checker-typed forms answering a `Cookie` named and encoded as the record's decoder reads it; amends ADR 0806 E

**Context.** A cookie was written by hand-typing its name and its wire text, and nothing tied either to the record that reads it. `Cookie { name: "theme", value: "dark" }` compiles for a `Theme` field the decoder reads only as `Dark`, and a renamed field leaves the write valid and wrong; the fault surfaces as a `400` on the next request, in another handler (#3446).

**Decision.**

- **A. Surface.** `<CookieRecord>.<field>.cookie(value)` answers a `Cookie` named by the field's wire name (`@name`, else the field) with the value encoded as the decoder reads it, every attribute the `Cookie` default; `<CookieRecord>.<field>.expire()` answers the matching deletion (empty, `Max-Age=0`, epoch `Expires`). Both compose with `withCookie` and the `Cookie { ...spread }` literal, so attributes have no second vocabulary. The maintainer-approved spelling (the issue's amendment) replaces `Prefs.cookie(field: value)`, which would have needed labelled call arguments, a grammar change the proposal did not scope.
- **B. Recognition.** The checker recognises the full form `Record.field.method(...)` as a kernel call beside `Actor.issue`, ahead of cross-context resolution; `Record.field` alone is not a value, and a record's own static `cookie` or `expire` takes precedence. The field segment is recorded as an ordinary field reference (rename, go-to-definition).
- **C. Which records.** The record must be the `cookies` parameter of an `on http` handler of the unit that writes the cookie, so the decoder the write agrees with is local and checkable. A `List` field is refused (a repeated name is several `Set-Cookie` lines).
- **D. Encoding.** The decoder's leaf rules, inverted: an `Int` its digits, a `Bool` `true`/`false`, an enum its variant's name, a `String`, refined or opaque type over one, and a `Decimal` their text; an `Option[T]` field takes a `T`. The leaf kind is the very `query_leaf_kind` the decoder's field spec uses, so the two read one table. Lowering inlines the conversion rather than adding a runtime helper: the table is a few expressions and lives in the emitter beside the kind it keys on.
- **E. Diagnostics.** `bynk.http.cookie_field_not_cookie_record`, `_unknown`, `_arity` and `_list`, each an error; a value of the wrong type is the existing `bynk.types.type_mismatch`.

**Consequences.** No grammar change and no change to `Cookie`, `withCookie`, `clearCookie` or `SignedCookie`; every existing program emits as before, and a program that did not use the new forms is unaffected. The lints on the untyped form, the prefix and attribute checks on the typed name, and the `SignedCookie` write refusal follow in the next increments.
