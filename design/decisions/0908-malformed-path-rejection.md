# 0908 — A path that cannot be percent-decoded is a 400 MalformedPath, not a fault

- **Status:** Accepted (v0.655.1)

**Context.** `matchPath` decoded each path-parameter segment with `decodeURIComponent`, which throws on malformed percent-encoding. The throw reached the Worker entry's `catch`, which answered `500`, logged a fault and reported it, so any client could raise server-fault alerts with `GET /orders/%E0%A4%A`.

**Decision.** (A) The path is decoded once, inside the matcher: a path whose shape a route would match but whose segments do not decode answers `400` with `{ "kind": "MalformedPath", "details": "path segment N is not valid percent-encoding" }` before the route's actor, body or handler is touched; no route can match an undecodable path. The decoding is done by a `decodePath` helper that returns `null` on failure, and the matcher reports the failure to the route instead of throwing, so the refusal is stamped with the matched service's `on rejected`, CORS and security policy like the other router rejections. (B) `MalformedPath` is a new `BoundaryRejection` variant carrying a `detail` that names the segment's position, never its content; `on rejected` receives it, and a `system` case sees `Rejected(MalformedPath(_))`. An exhaustive `match` on `BoundaryRejection` gains an arm. (C) It is not a fault: it is not logged and not reported.

**Consequences.** Anyone can no longer drive fault alerts with a malformed path. Existing tests with an exhaustive `match` over `BoundaryRejection` need the new arm. A request with the wrong method to a route whose path is malformed still answers `405`, since the method is judged first.
