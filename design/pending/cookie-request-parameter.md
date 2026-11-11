---
level: minor
changelog: A handler reads cookies through a typed `cookies: T` parameter and sets them with `Cookie`, `SameSite`, `withCookie`, `clearCookie` and `cookies()` on an `HttpResult` (#3277, slices 1 and 2; #3181 stays open for signing and the actor scheme)
probes: emit_diagnostics, ts_writes, ts_any, diagnostic_coverage, explained_codes
---

## ADR: cookie-request-parameter
title: A handler reads cookies through a `cookies: T` record parameter and sets them with `withCookie`
summary: How a route declares the cookies it reads, how the Cookie header parses, and how a result attaches the cookies it sets

**Context.** There was no cookie support: a handler could read the raw `Cookie` header through `headers.get`, with no parsing and no validation. The typed `query` (#2854) already reads a flat key/value string into a record through the record's own codec, and `@form` (#3276) generalised that reader. Proposed in #3277 (slices 1 and 2; the compile-time refusals follow).

**Decision.**
- **A: a declared `cookies` parameter,** beside `headers`, `query` and `body`, in its typed-record form: the router decodes and validates it before the handler runs, so the sessions example's refined `Token` is checked before any cache lookup. Rejected: a `cookie(name)` accessor on `RequestHeaders`, which declares nothing in the route's signature and has no typed form. The raw `RequestCookies` view the proposal names (`get`, `all`, `names`) is **not** in this slice: it is a new built-in type with the plumbing `RequestQuery` has (about a hundred sites), and the typed form covers the sessions case.
- **B: RFC 6265 §5.4 parsing, no decoding.** Pairs split on `;`, each on its first `=`, trimmed, one pair of double quotes dropped; a pair with no `=` or an empty name is ignored; values are never percent-decoded, because a cookie value is opaque and the writer emits what it is given. A name sent twice fills a `List` field and is otherwise a `StructuralMismatch` (never first-wins).
- **C: one reader.** The cookies feed the same `fieldsObject` reader as `query` and `@form` (the typed-`query` field set, by the same leaf rules); rejections name `cookies.<field>`. An empty cookie (`a=`) is a value, as for a query.
- **D: tests.** The system-tier driver sends a typed record as the `Cookie` header it encodes to, and a `Wire(…)` argument as the header text.
- **E: setting cookies.** `bynk` exports a `Cookie` record (`name`, `value`, `maxAge`, `expires`, `domain`, `path = "/"`, `secure = true`, `httpOnly = true`, `sameSite = Lax`, `partitioned = false`) and `SameSite`. `HttpResult` gains `withCookie(c)`, `clearCookie(c)` and `cookies()`; the result carries its cookies and the router appends one `Set-Cookie` per cookie. The safe attributes are the defaults, so a session cookie is `Cookie { name, value }`.
- **F: the runtime is the backstop.** `serialiseCookie` refuses a name that is not a token, a value with a character a cookie cannot carry, an unsafe `Domain`/`Path`, an out-of-range `Expires`, a `__Host-`/`__Secure-` name without its required attributes, `SameSite=None` or `Partitioned` without `Secure`, and a header over 4096 bytes. The request faults (500) rather than send a malformed or unsafe header.
- **G: caching and validators.** A response that sets a cookie is one user's, so `@cache(scope: public)` is downgraded to `private` at runtime when a `Set-Cookie` is present, and a conditional-`GET` `304` carries the `Set-Cookie` of the response it replaces.
- **Deviations from the proposal.** The `SameSite` variant is `Unrestricted`, not `None`, so it does not clash with `Option`'s `None`. Deletion is `clearCookie(c)`, which sends the cookie empty with `Max-Age=0` and an epoch `Expires` (it keeps the cookie's `secure`/`sameSite`, so clearing a `__Host-` cookie built with `secure: false` faults), rather than `Cookie.expire`, because records have no static or instance methods.

**Consequences.** A browser-facing service can read session cookies at the boundary. A cookie is client input, so every field is still validated; a handler can now set cookies, with unsafe combinations refused at runtime. A result that sets cookies carries a `cookies` field, which `cookies()` reads in tests. The compile-time refusals and warnings for unsafe attribute combinations and cache interactions, the `examples/sessions` conversion and the raw `RequestCookies` view follow in later slices of #3277.
