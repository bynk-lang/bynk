---
level: minor
changelog: An HTTP service or route can write response `Option`s as plain JSON (`v` / `null`) with `json { options: plain }` and `@json`
probes: diagnostic_coverage, emit_diagnostics
---

## ADR: plain-json-http-options
title: HTTP response bodies can opt into plain JSON for `Option`
summary: A `json { options: plain }` section and `@json` annotation write `Some(v)` as `v` and `None` as `null` in response bodies only

**Context.** Bynk writes an `Option[U]` as `{"kind": "Some", "value": v}` or `{"kind": "None"}` ([ADR 0450](0450-lenient-option-decode.md) made the decoder lenient but kept the encoder tagged so `decode(encode(x)) == x` and contract hashes hold). That is right for Bynk talking to Bynk, but every non-Bynk consumer of an HTTP API (`curl | jq`, a dashboard) must special-case it. Proposed in #2858.

**Decision.**

- **A, where it is chosen.** A service-level `json { options: tagged | plain }` section, in header position after `limits { }` (a contextual keyword, parsed like `cors`/`security`/`limits`), sets the default for the service's response bodies. A per-route `@json(options: tagged | plain)` overrides it. The default stays `tagged`, so nothing changes unless asked. #918 (envelope refinement) has not landed, so the block form stands; the surface can move there later.
- **B, the ambiguous shape.** `Option[Option[T]]` reachable from a plain route's response type (records, sums, `List`/`Set`/`Map`/`Result`, generic arguments after substitution) is refused as `bynk.json.nested_option_plain`: `Some(None)` and `None` would both be `null`. This keeps plain encoding injective. `Option[()]` (and any inner type that itself encodes as `null`) is not covered by this rule and is left as a known gap.
- **C, `None`.** `None` is written `null`, never an absent key, so shapes stay stable and OpenAPI can say `nullable`.
- **D, scope.** HTTP `Ok`/`Created`/`Accepted` response bodies only. Cross-context calls, queues, agent state, `Json.encode`, SSE events and contract hashes keep the tagged form.
- **E, OpenAPI.** `bynk openapi` describes a plain route's `Option` as nullable.

**Mechanism.** One codec with a mode flag, not a second codec. A plain route's body serialiser is wrapped in the runtime's `withPlainJson`, which sets a flag (`globalThis.__bynkPlainJson`) for the one synchronous serialisation and restores it. The generated `__serialise_Option_*` encoder reads the flag and writes `v` / `null` or the tagged object. The plain branch is emitted only in a project that has a plain route, so every other project's output is byte-for-byte unchanged. The mode resolves per route in the emitter from `effective_json_options` (route `@json`, then service `json { }`, then tagged).

**Consequences.** A Bynk client reads a plain response through the lenient decoder, so a round trip stays closed; because that decoder also accepts the tagged form, the wire shape is asserted on the raw bytes by `http_plain_json_behaviour`. Hover, completion and the TextMate/tree-sitter grammars know the new section and annotation; signature help is unchanged (neither form is a call). Semantic tokens are unchanged: `@json` rides the generic handler-annotation tokens and `json { }`, like `limits { }`, is a lexical keyword. `bynk openapi` writes a plain route's `Option` as `anyOf [U, null]`; a type used by both a tagged and a plain route renders as two components (`Name` and `Name.plain`), and request bodies stay tagged. No hover shows a route's wire shape today, so the response-type hover delta is the `plain` marker on the service hover and the `@json` hover.
