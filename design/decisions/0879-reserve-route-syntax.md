# 0879 — `*`, `{` and `}` are reserved in route segments before 1.0

- **Status:** Accepted (v0.640)

**Context.** A route segment that is not `:name` was matched as a literal,
whatever it contained, so `on GET("/files/*key")` compiled and answered only
the literal path `/files/*key`. A rest segment (#3752) is likely after 1.0,
and every common spelling of one (`*rest` in Rails and Express 4, `:rest*` in
path-to-regexp, `{*rest}` and `{...rest}` in ASP.NET and Astro) would silently
change the meaning of such a route: a breaking change to the frozen surface,
or else a spelling chosen to avoid existing routes rather than on its merits.

**Decision.** (#3753) `*`, `{` and `}` in any segment of an `on <METHOD>(…)`
route are a compile error, `bynk.http.reserved_route_syntax`, with a note
giving the workaround: take the segment as a `:name` parameter and check it in
the handler.

- *Which characters:* `*`, `{` and `}`, which cover the spellings in common
  use. `?` cannot appear in a path, and `+` is common in real paths, so
  neither is reserved.
- *An error, not a warning:* a program that compiles with a warning today
  would still change meaning later, so a warning would not protect the
  surface.
- *Inside `:name` too:* `:key*` is refused as well, so `*` is reserved as a
  suffix on a parameter name and not just on its own. A refused segment is
  not also reported as a parameter (`:key*` gives one diagnostic, not a
  second `bynk.http.unbound_path_param`).

WebSocket `on open` routes and `routes` in `bynk.toml` (Cloudflare's patterns,
where `*` is already a wildcard) are out of scope.

**Consequences.** Whichever spelling #3752 settles on can be added as a purely
additive change. A route that used one of these characters literally no
longer compiles; none existed in the examples, the Book or the test corpus.
The checker gains one loop over each segment; the grammar, the emitter and
the runtime are unchanged.
