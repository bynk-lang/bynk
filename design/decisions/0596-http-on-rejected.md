# 0596 — Rendering the router's rejections

- **Status:** Accepted (v0.397)

**Context.** A service that implements an external HTTP contract
(`design/pending/http-external-contract.md`) can answer its own errors in the
contract's shape with `RawError`. But some refusals never reach a handler. The
router answers a body that fails to parse or validate (`400`), a method the path
doesn't declare (`405`), a body over its cap (`413`) and a `@raw` route's other
content type (`415`) itself, as `{ "kind": …, "details": … }` or no body. A
`Signature` seam's `401` is the same. The first-party registry's publish API
(slice R2) must answer these in its protocol's `{ "error": code, "message": text }`
shape too, so a contract can't be implemented fully without a say in them. This is
decision C of the accepted proposal (#2341).

**Decision.**

- **The handler.** A `from http` service may declare one
  `on rejected(why: BoundaryRejection) -> HttpResult[()]` (or
  `Effect[HttpResult[()]]`). `BoundaryRejection` is the sum a system-tier test
  already observes (`MalformedJson`, `StructuralMismatch`, `RefinementViolation`,
  `PayloadTooLarge`, `UnsupportedMediaType` with a `detail`; `MethodNotAllowed`,
  `Unauthorized`). It can now be named as this parameter's type, and nowhere that
  stores or sends it (`bynk.types.rejection_at_boundary`). The handler has no `by`,
  because it runs before any actor is known, and no `given`, because it renders a
  body and nothing more. A service-level `by`/`given` default does not apply to it.
  Its shape is checked (`bynk.http.rejected_signature`), and so is its uniqueness
  (`bynk.http.duplicate_rejected`).
- **The status is kept.** The router builds its default response as before. With
  a hook, it passes the default through the runtime's `renderRejection` before
  stamping CORS and security headers. That reads the rejection back from the
  response, calls the hook, and uses the answer only when it is a `RawError` whose
  status is the rejection's own. The default's other headers (a `405`'s `Allow`)
  are kept. Anything else (another variant, another status, a fault in the hook)
  sends the default response and logs why. So the hook can change a rejection's
  body, but never its status or whether it happened.
- **Where it applies.** Every response the router's dispatch synthesises for one of
  the service's routes: the `413` (fast-reject and counted read), the `415`, the
  `400`s for the body and path parameters, a `Signature` seam's `401`, and the path
  fall-through's `405` (whose owning service's hook is used). A `204` to `OPTIONS`
  is not a rejection and passes through.
- **A shared path's `405`.** A `405` belongs to a path, not a route. When two
  `from http` services in one context share a path, the `405` uses the hook of the
  service declared first. That is the same rule its CORS and security headers
  follow. A service with `on rejected` and no routes compiles, and its hook is
  never called.

**Deviations from the accepted proposal.**

- **Scope.** The `401`s that the Bearer and OIDC seams return, and the body refusals
  of a route whose `by` is a sum of actors, are built inside the compose wrappers,
  not by the router. They are not rendered through the hook yet. That is a
  follow-on, so a contract whose authentication is an actor's still answers those
  in Bynk's shape. (The registry's own tokens are read through `headers`, not an
  actor, so R2 is not blocked.)
- **No unit-tier address.** The proposal's `api.rejected(PayloadTooLarge("…"))`
  needs `BoundaryRejection` constructors. Its variant names collide with
  `HttpResult`'s (`PayloadTooLarge`, `Unauthorized`, `MethodNotAllowed`, …), so
  this increment adds none. The hook is tested at the system tier, through the real
  router.
- **The system tier reads a rendered rejection by its status.** A hooked
  rejection's body no longer carries the router's `kind`, so the driver decodes it
  as it does any response: a rendered `413` reads as `Handled(PayloadTooLarge(m))`.
  A `405` keeps its `Allow` and still reads as `Rejected(MethodNotAllowed)`.

**Consequences.** A `from http` service can implement an external contract's
rejection shape. Without `on rejected`, the emitted router is unchanged.
