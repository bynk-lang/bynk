---
level: minor
probes: emit_diagnostics
changelog: An `on ANY(…)` pass-through route returning `Effect[PassThrough]`, with `bynk`'s `InboundRequest`, `Forwarded`, `PassThrough`, `UpstreamError` and an `Upstream` capability whose `@origin` is declared, now type-checks; its router and the upstream exchange arrive in the next increment, so a build with one is refused (`bynk.http.pass_through_not_emitted`); experimental, behind `bynk.http.ANY` and the `bynk` items in `[unstable] allow`. A bare variant that several sums share is now picked by a body's `Effect[T]` as by a plain `T`, so an email handler's `Forward` stays `EmailVerdict.Forward` beside `PassThrough.Forward`
---

## ADR: pass-through-route-form
title: A pass-through route is `on ANY(…)` returning `bynk`'s `Effect[PassThrough]`, forwarding through a declared `Upstream`
summary: `ANY` routes, the `bynk` types `InboundRequest`, `Forwarded`, `PassThrough`, `UpstreamError` and the `Upstream` capability with `@origin`/`@timeout`; checked now, built next

**Context.** The pass-through HTTP track (`design/tracks/pass-through-http.md`, spine
#3752) puts reverse-proxy routes in the ordinary service tier (D1). Slices 1 and 2 gave a
route a `*name` rest and a handler its request's headers. This increment, the first half
of slice 3 (split as the track's §15 allows), adds the route form and its types. The
router behaviour it suspends (D9) and the `Upstream` exchange are the second half.

**Decision.** This records D7, D8, D10, D11 and D13.

- **The route form (D7).** `on ANY("<route>")` is new syntax in the method position:
  `HttpMethod::Any`, `IrHttpMethod::Any`, and `ANY` in tree-sitter's `http_method`. An
  `ANY` route must return `Effect[PassThrough]`, and no other route may
  (`bynk.http.pass_through_signature`). It takes only path parameters, a rest,
  `request: InboundRequest` and `cf`. `body`, `@raw`, `@form` and `@multipart` are
  refused, because the body belongs to the upstream, and `headers`, `query` and
  `cookies` are read through `request`.
- **The response (D8).** `PassThrough` is `Forward(response: Forwarded)` or
  `Respond(result: HttpResult[()])`. `Forwarded` is opaque with no constructor, so only
  `Upstream.forward` makes one, and it has `status()` and `header(name)`.
- **CORS (D10).** A service that declares an `ANY` route may not declare `cors { }`
  (`bynk.http.cors_with_pass_through`). `@cache` is refused on an `ANY` route
  (`bynk.http.cache_on_pass_through`, D9's row).
- **The request (D11).** `InboundRequest` is opaque, with `method()`, `headers()` (the
  `RequestHeaders` view) and `query()` (the raw query string). Only an `ANY` route takes
  one, named `request` (`bynk.http.inbound_request_not_pass_through`).
  `InboundRequest` and `Forwarded` live for their request: a record or sum may hold one,
  as for a `Stream`, but no value holding one is stored, sent or returned across a
  boundary, and none has a codec (`bynk.types.inbound_request_at_boundary`). `bynk`'s
  own `PassThrough`, which holds a `Forwarded`, is the one return it is allowed in.
- **The upstream (D13).** `Upstream` is an instantiable capability of the portable
  `bynk` surface. `@origin` is required: a serialised origin, read by #4504's `cors`
  origin reader, on `https`, or on `http` for a loopback host only, given as a literal
  or a `String` constant. `@timeout` is optional, in whole seconds. An instance reaches
  its origin with `fetch`, so it has no Worker binding.
  `forward(request, path) -> Effect[Result[Forwarded, UpstreamError]]`, where
  `UpstreamError` is the extensible `Timeout`, `Unreachable` or `InvalidPath`.
- **Ownership (D5, the static part).** An `ANY` route takes part in route order with
  every ordinary route, whatever its method. It owns a request only where it is strictly
  more specific than all of them; at equal shape the ordinary route wins. An `ANY` route
  that ordinary routes always beat is `bynk.http.unreachable_route`.
- **Not built yet.** Every build that reaches an `ANY` route is refused, for every
  target (`bynk.http.pass_through_not_emitted`), until the router lands. A build that
  dropped the routes could deploy a Worker that answers `404` where the program says it
  forwards. `bynk check` accepts the route. `bynk client` and `bynk openapi` leave it
  out, named on stderr.
- **Experimental.** The form is gated by `bynk.http.ANY` in `[unstable] allow`, an entry
  in `bynk-std`'s `BUILTIN_EXPERIMENTAL`, because it is syntax. The types and the
  capability are gated by the `bynk` unit's ordinary experimental list.

**Deviation from the track.** §11.2 writes only `consumes bynk { Upstream }` and uses the
other names bare, as if they were language built-ins. They are instead types of the
`bynk` unit, exactly as `Secret` and `Authorization` are: opaque types with kernel
methods, found by their declaration, so a program's own `InboundRequest` is just a type.
A program imports them:
`consumes bynk { Upstream, InboundRequest, PassThrough, UpstreamError }`. A built-in
`Ty` per type would have touched about sixty files of match arms each. The coordinator
ruled for the unit types in review (L19 slice 3a). Variant names still resolve as for
any consumed sum. A program written before this keeps compiling with its own `Forward`
or `Respond`, or with bare `Timeout` and `Unreachable` (which `FetchError` and
`CallOutcome` already declare), even consuming the whole of `bynk`. A program that
imports `PassThrough` beside its own `Forward` qualifies the one it means, unless an
expected type picks it.

`bynk.cloudflare`'s `EmailVerdict` also declares `Forward`, so an email handler that
consumes the whole of `bynk` would have seen two. The expected type already settles a
shared variant name (#3344), but a handler body returning `Effect[EmailVerdict]` was
checked against the `Effect`, which names no sum. A variant is never itself an
`Effect`, so the expected type's `T` now decides, as a plain `T` does: the handler's
bare `Forward` is the verdict's, as before, and a pass-through route's bare `Forward`
is `PassThrough`'s.

Rejected:
- A `Forward` variant on `HttpResult`, which would break every exhaustive `match` (§12).
- `@passThrough` on an ordinary method, because `HEAD` and `OPTIONS` aren't declarable.
- Language built-in types, for the cost above.
- Emitting nothing for an `ANY` route until the router lands, for the reason above.

**Consequences.**
- A pass-through program can be written and checked today, and its port of the blog
  proxy (§11.2) checks.
- The next increment builds it, retiring `bynk.http.pass_through_not_emitted`, and
  supplies `UpstreamProvider` in each platform binding. It is a refusing stub until then,
  unreachable because the only `InboundRequest` comes from an `ANY` route.
- `vscode-bynk`'s grammar needs `ANY` in its method alternation.
