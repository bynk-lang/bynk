---
level: minor
probes: emit_diagnostics, ts_writes, diagnostic_coverage, explained_codes
changelog: A route's last segment may be a `*name` rest segment, which matches zero or more remaining segments and binds them, still percent-encoded, as a `String` (`on GET("/files/*key")`); it is the weakest segment when routes are ordered, never answers under `/_bynk/`, and is left out of `bynk client` and `bynk openapi`; experimental, behind `bynk.http.rest` in `[unstable] allow`. Two handlers of a service whose emitted names would collide (`/a-b` and `/a_b`) are now `bynk.http.handler_name_collision`
breaking: Two HTTP routes of one service whose emitted handler names collide (`GET /a-b` beside `GET /a_b`, both `http_GET_a_b`), where one handler silently answered both paths, are now refused as `bynk.http.handler_name_collision`; spell one route differently, or move it to another service
---

## ADR: rest-segments
title: A route's last segment may be a `*name` rest that binds the raw rest of the path
summary: `*name`, last and whole, matches zero or more segments, binds a raw `String`, and is the weakest segment; amends ADR 0879 and ADR 0914

**Context.** A Bynk route matched a fixed number of segments, so a service couldn't
serve R2 keys that hold `/`, put a single-page app's fallback at the root, or (later
in the pass-through HTTP track, `design/tracks/pass-through-http.md`, spine #3752)
forward a whole subtree to another site. ADR 0879 reserved `*`, `{` and `}` in routes
so that such a form could be added without changing any existing route's meaning,
and ADR 0914 set the route order: segment by segment, a literal before a `:name`.

**Decision.** This records the track's decisions D2 to D6.

- **Syntax (D2).** A route's last segment may be `*` then an identifier. It is a whole
  segment: `*key.txt`, and a `*` anywhere else, stay `bynk.http.reserved_route_syntax`.
  A bare final `*` is `bynk.http.rest_unnamed`.
- **What it binds (D3).** The rest binds a handler parameter of its name, which must be
  `String` (`bynk.http.rest_param_type`). Its value is the rest of the path *as
  received*: sliced from the raw path, never rebuilt from segments. So it is still
  percent-encoded, starts with `/` unless it is empty, and keeps a trailing slash and
  empty segments. The query is not part of it. For `/blog/*path`, `/blog` binds `""`,
  `/blog/` binds `"/"` and `/blog/a%2Fb` binds `"/a%2Fb"`. A handler decodes it with
  `percentDecode()`. Keeping it raw is what lets a later pass-through route send
  exactly what it received.
- **Edge cases (D4).** The segments before the rest are matched decoded, as today. A
  malformed escape inside the rest isn't a `400`: the handler judges it. A WHATWG URL
  never holds a dot segment, so the rest can't hold one. The router still answers
  `400 MalformedPath` if one appears, as defence in depth. A rest matches **zero** or
  more segments, so `/blog/*path` answers `/blog` itself.
- **Order (D5, amending ADR 0914).** A rest is the weakest segment: at the first
  position where two routes differ, a literal beats a `:name`, a `:name` beats a rest,
  and the end of a route beats a rest. So `GET /blog` beats `GET /blog/*path` for
  `/blog` (and `/blog/`), while `GET /a/:id` and `GET /a/*x` share `/a/<x>` and
  `/a/1/2` goes to the rest. `shadowed` covers rests: two rests at one position leave
  the second spelt unreachable.
- **The reserved prefix.** A rest never matches a path under `/_bynk/`. The runtime
  dispatches the `/_bynk/` prefixes it serves before any route, but a root catch-all
  would otherwise take the others.
- **Code (D6).**
  - The router binds `__m` for a rest route, as for a `:name`.
  - A rest becomes `Rest_<name>` in the emitted handler name (`http_GET_files_Rest_key`).
  - `bynk client` and `bynk openapi` leave a rest route out, named on stderr. An
    OpenAPI path parameter can't hold `/`, and a client would encode the slashes.
  - A test case addresses a rest route by its pattern, then the raw rest
    (`files.GET("/files/*key", "/a/b.png")`). The system-tier driver sends that rest
    as written.
- **Handler names.** The emitted handler name was not one-to-one: `/a-b` and `/a_b`
  were both `http_GET_a_b`, and one handler answered both paths. The naming scheme
  moves into `bynk_check::emitted_names`, and two handlers of one service whose names
  would collide are `bynk.http.handler_name_collision`.
- **Experimental.** The form needs `bynk.http.rest` in `[unstable] allow`. That is an
  entry in `bynk-std`'s `BUILTIN_EXPERIMENTAL`, the list of experimental built-ins.

Rejected:
- **Spelling.** `:name*`, `{*name}` and `{...name}`. The first reads as a modifier and
  invites `+` and `?`; the other two spend the braces.
- **What it binds.** A decoded `List[String]`, which loses the trailing slash, empty
  segments and `%2F`. A `Path` type, a new thing to learn for one property the router
  already guarantees.
- **Matching one or more segments**, which needs a second route for the bare prefix. A
  literal `/blog` route also answers `/blog/` (empty segments don't count) and would
  beat the rest there.

**Consequences.**
- A service can serve keys with slashes, a single-page app's fallback, or a whole
  subtree. Every route that compiled before means the same: `*` was reserved, and the
  ordering rule only gains a third rank.
- The `405`/`Allow` union, the CORS preflight and `bynk.asset.*_shadows_route` read
  rests through the same `route_order` functions, so they cover rest routes with no
  separate rule.
- The new name-collision check refuses a program that compiled before but silently
  served two routes from one handler. That is the one break; see `breaking:`.
