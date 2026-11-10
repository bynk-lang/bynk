---
level: minor
changelog: A handler reads cookies through a typed `cookies: T` parameter, decoded from the `Cookie` header and validated at the boundary like a typed `query` (#3277, slice 1; #3181 stays open for signing and the actor scheme)
probes: emit_diagnostics, ts_writes, ts_any, diagnostic_coverage, explained_codes
---

## ADR: cookie-request-parameter
title: A handler reads cookies through a `cookies: T` record parameter
summary: How a route declares the cookies it reads, how the Cookie header parses, and how cookies map to the record

**Context.** There was no cookie support: a handler could read the raw `Cookie` header through `headers.get`, with no parsing and no validation. The typed `query` (#2854) already reads a flat key/value string into a record through the record's own codec, and `@form` (#3276) generalised that reader. Proposed in #3277 (slice 1; writing cookies and their compile-time refusals follow).

**Decision.**
- **A: a declared `cookies` parameter,** beside `headers`, `query` and `body`, in its typed-record form: the router decodes and validates it before the handler runs, so the sessions example's refined `Token` is checked before any cache lookup. Rejected: a `cookie(name)` accessor on `RequestHeaders`, which declares nothing in the route's signature and has no typed form. The raw `RequestCookies` view the proposal names (`get`, `all`, `names`) is **not** in this slice: it is a new built-in type with the plumbing `RequestQuery` has (about a hundred sites), and the typed form covers the sessions case.
- **B: RFC 6265 §5.4 parsing, no decoding.** Pairs split on `;`, each on its first `=`, trimmed, one pair of double quotes dropped; a pair with no `=` or an empty name is ignored; values are never percent-decoded, because a cookie value is opaque and the writer emits what it is given. A name sent twice fills a `List` field and is otherwise a `StructuralMismatch` (never first-wins).
- **C: one reader.** The cookies feed the same `fieldsObject` reader as `query` and `@form` (the typed-`query` field set, by the same leaf rules); rejections name `cookies.<field>`. An empty cookie (`a=`) is a value, as for a query.
- **D: tests.** The system-tier driver sends a typed record as the `Cookie` header it encodes to, and a `Wire(…)` argument as the header text.

**Consequences.** A browser-facing service can read session cookies at the boundary. A cookie is client input, so every field is still validated; nothing here sets a cookie. Setting cookies (`Cookie`, `withCookie`, deletion), the compile-time refusals of unsafe attribute combinations, the `examples/sessions` conversion and the raw view follow in later slices of #3277.
