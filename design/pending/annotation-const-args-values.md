---
level: minor
changelog: A constant of the exact base type is accepted in `@ttl`, `@retain`, `@cache(maxAge:, immutable:)`, `@limit`, `@schema`, `@raw(contentType:)` and `@example`
---

The remaining value-taking annotations join `@name` on the shared resolution pass (ADR "A constant in a literal-only annotation argument", from the previous increment): each is a row in the pass's rule table naming the annotation, the argument's label and the base type its constant must have. `@example` takes the base of the refinement it follows. Keyword arguments (`@cache(scope:)`, `@json(options:)`), `@indexed(by:)` and the value-less markers are unchanged: there is nothing to substitute. The single-file path (`bynk check file.bynk`, the playground, the editor's single-file mode) runs the same pass.
