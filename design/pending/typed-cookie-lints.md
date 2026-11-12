---
level: minor
changelog: An untyped `Cookie` literal named for a cookie record's field is warned about and its undecodable value refused; the `SignedCookie` cookie cannot be written by hand or from a record, and the typed name is judged by the prefix rules
breaking: A `Cookie { name, value }` literal named for a field of a `cookies` record whose literal `value` that field's decoder rejects (`value: "dark"` for an enum read as `Dark`) is now the error `bynk.http.cookie_value_undecodable`, and an untyped `Cookie` literal named like a `SignedCookie` actor's session cookie is `bynk.actor.signed_cookie_write_literal`; fix the value, write it with `Record.field.cookie(v)`, or write the session cookie with `Actor.issue`/`Actor.expire`.
---

## ADR: typed-cookie-lints
title: The untyped cookie's lints, the typed name's prefix rules and the SignedCookie write refusals
summary: Warns where an untyped `Cookie` could be written from its record, refuses a value its decoder rejects, applies the cookie lints to the typed name, and keeps the `SignedCookie` cookie to its actor; extends ADR 0821's read-side refusal

**Context.** A typed write (the previous increment) removes the second spelling of a cookie's name and value, but the untyped `Cookie { name, value }` stays, and nothing yet related it to a record, applied the `__Host-`/`__Secure-` rules to a typed name, or kept the credential cookie of a `SignedCookie` actor out of hand-written and record-built writes (#3446).

**Decision.**

- **A. `cookie_untyped_has_field` (warning).** An untyped literal whose name equals the wire name of a field of a cookie record of the same unit. The note names the typed write; the quick-fix (machine-applicable) rewrites a literal that is only a name and a value, when the value provably decodes to the same text (an `Int`, `Bool`, enum or plain `String` field). A literal with attributes is told to spread the typed cookie instead.
- **B. `cookie_value_undecodable` (error).** In the same situation, a literal `value` the field's decoder rejects: an enum other than its exact variant name, an `Int` that is not a safe integer's digits, a `Bool` other than `true`/`false`. It refuses code that is already wrong at run time (a `400` on every later request), but is a new error on code that compiled, so the increment carries a `breaking:` line. A computed value, and a refined, opaque or `Decimal` field, are not judged.
- **C. The typed name is judged.** `lint_cookie_literal` also reads `Cookie { ...R.f.cookie(v), … }` and a bare typed cookie, against the field's resolved wire name: the `__Host-`/`__Secure-` prefix rules, the `domain` and `partitioned` checks and the rest apply as to a literal name.
- **D. `SignedCookie` (ADR 0821).** A cookie whose name is a `SignedCookie` actor's session cookie in the unit may be written only by the actor: a typed write (`bynk.actor.signed_cookie_write_directly`, the twin of `signed_cookie_read_directly`) and an untyped literal (`bynk.actor.signed_cookie_write_literal`) are both errors. The credential is then always signed with the actor's key, expires, and sits under its CSRF seam.

**Consequences.** Both lints and the refusals read facts the unit already holds (`CrossContextInfo::cookie_records` and `signed_cookie_names`); no grammar, emitter or runtime change.
