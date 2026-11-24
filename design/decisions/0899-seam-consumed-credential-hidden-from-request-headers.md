# 0899 — A seam's consumed credential header is omitted from the handler's `RequestHeaders`

- **Status:** Accepted (v0.648.1)

**Context.** A `SignedCookie` actor's credential is consumed at the seam and the handler's `cookies` and
`headers` views omit it (#3386). A `Bearer` or `Oidc` route's handler could still read the verified JWT
through `headers.bearer()`, so it could log, store or forward a live token, against the actor model's promise
that the handler sees only the identity (#3994).

**Decision.** The handler's `RequestHeaders` omits the header the route's seam consumed: `Authorization` on a
`Bearer` or `Oidc` route, the signature header on a `Signature` route (its timestamp header and the body stay
readable). The runtime's `requestHeaders` takes the cookie to hide and a list of headers to hide. On an actor
sum every member's credential header is hidden, because the entry builds the view before the sum's wrapper
resolves the matching member; this is a superset of "what the matched arm consumed" and cannot leak a
credential. Visitor routes consumed nothing and are unchanged. No `u.token()` member is added: forwarding a
token is not offered, and a service that needs it would have to be given an explicit, documented member.

**Consequences.** Handlers that read the token from `RequestHeaders` on a `by` route now see `None`. This is a
patch-level fix to a runtime view, with a `breaking:` note for the handlers that relied on it. The `ApiKey`
scheme (#3939) adopts the same rule for its header.
