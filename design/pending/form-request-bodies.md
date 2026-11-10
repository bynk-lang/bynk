---
level: minor
changelog: `@form` decodes an `application/x-www-form-urlencoded` request body into a typed record at the boundary, with the refinements, defaults and `Option`s of a JSON body (#3276, slice 1; #3180 stays open for streaming)
probes: emit_diagnostics, ts_writes, ts_any, diagnostic_coverage, explained_codes
---

## ADR: form-request-bodies
title: A `@form` handler annotation decodes a urlencoded form body into a record
summary: How a route declares a form body, how keys map to record fields, the form-specific leaf rules, and the 415 and 400 answers

**Context.** A handler's `body` was JSON, or with `@raw` the untouched octets; an HTML form post had to be hand-parsed from `Bytes`, losing the boundary's refinements, defaults and path-naming errors. A typed `query` (#2854) already reads a flat key/value string into a record through the record's own codec. Proposed in #3276 (slice 1 of #3180; slice 2, multipart, follows).

**Decision.**
- **A: a handler annotation.** `@form`, parallel to `@raw`, fixes the route's one media type, `application/x-www-form-urlencoded`; any other `Content-Type`, JSON included, is a router `415`, ahead of the auth seam and the body read. It excludes `@raw` (`bynk.http.body_kind_conflict`), and is refused on a `by` sum of actors, whose wrapper reads the body as JSON (`bynk.http.form_on_sum_route`). Rejected: a `Form[T]` wrapper (a new generic with its own boundary rules, and a second mechanism beside `@raw`), and negotiation on `Content-Type` (three wire contracts for one route).
- **B: one reader.** The body record's fields are the typed-`query` set (a `String`, `Int`, `Bool`, `Decimal`, enum, or refined or opaque type over one, optionally in an `Option` or a `List`); form keys are flat, a nested record is refused (`bynk.http.form_field_not_stringy`). The runtime's `queryObject` is generalised into one `fieldsObject` reader behind `queryObject` and `formObject`, so the leaf rules cannot drift. Rejections name `body.<field>`.
- **C: the browser's two quirks.** A `Bool` also reads `on`/`off` (a checkbox's default value) and an unchecked box sends nothing, so a `Bool` with no default is a warning (`bynk.http.form_bool_without_default`); an empty value (`name=`) is absent for an `Option` and for every non-`String` leaf, and stays `""` for a `String`. A repeated key on a non-`List` field is a `StructuralMismatch`, as for `query`.
- **D: strict leaves.** An `Int` reads `-?[0-9]+` within the safe-integer range; no locale, no `+`, no whitespace trimming. Decoding is the URL standard's: UTF-8, `+` as a space, an invalid percent-sequence as U+FFFD. A `charset` parameter other than UTF-8 is a `415`.
- **E: tests.** The system-tier driver sends a typed `body` as the form its record encodes to (`__queryString` without the `?`), and a `Wire(…)` body as a form too, so a case probes the decoding rules with browser text. This differs from `@raw`, where `Wire` is `text/plain` to probe the `415`; the proposal had `@form` follow `@raw` here, but a form route's value is in probing its field rules, which `text/plain` would make unreachable. The `415` is covered by the runtime unit tests of `formMediaTypeMatches`.
- **F: platform.** Verified on `workerd` that `Request.formData()` parses both content types, throws `TypeError` on a malformed body, and reads a `filename=""` file part as an empty string where Node's undici gives an empty `File` (relevant to slice 2). `@form` itself uses `URLSearchParams`, not `formData()`.

**Consequences.** An HTML form can post straight to a typed route. `@form` needs no cap, but a public form should carry `@limit`/`limits`. `Form.decode`/`Form.encode` (the same mapping as library calls) and multipart follow in later slices of #3276. No existing route changes behaviour.
