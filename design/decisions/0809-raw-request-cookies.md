# 0809 — A handler reads a prefixed cookie through a raw `cookies: RequestCookies` view

- **Status:** Accepted (v0.554)

**Context.** The typed `cookies` record (ADR 0806) cannot name a cookie such as `__Host-sid`: a hyphen is not legal in a record field name. So a handler could set a `__Host-` or `__Secure-` cookie but could not read one back. Before this decision `examples/sessions` therefore used a plain `sid`; a general field-to-cookie-name mapping for the typed form is proposed separately as #3343.

**Decision.** `cookies: RequestCookies` is the second form of ADR 0806's Decision A: a built-in, read-only view with `get(name) -> Option[String]`, `all(name) -> List[String]` (header order) and `names() -> List[String]`. Parsing is the typed form's (RFC 6265 §5.4, no decoding), so a handler builds its own refined values (`Token.of(raw)`). Tests build one with `RequestCookies.of(["__Host-sid=abc"])` or `RequestCookies.empty()` at every tier; an entry without `=` is dropped, as the server drops it from a header. It follows the `RequestQuery` precedent (a built-in type with its own TS shape) and adds two diagnostics: `bynk.http.cookies_not_http` (on anything but an `on http` handler's `cookies` parameter) and `bynk.types.request_cookies_at_boundary` (it cannot be stored, sent or returned). `@cache(scope: public)` is still refused on a handler that takes it.

**Consequences.** A prefixed cookie is readable, and `examples/sessions` sets and reads `__Host-sid`. The raw view leaves validation to the handler. Closes #3277; a signed or actor-backed cookie scheme (#3181) is separate.
