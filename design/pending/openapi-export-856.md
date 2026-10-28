---
level: patch
changelog: "`bynk openapi` writes an OpenAPI 3.1 document for a project's `from http` services, derived from the routes, handler params, boundary types and refinements"
---

## ADR: openapi-export
title: bynk openapi — the compiler publishes its HTTP contract as OpenAPI 3.1
summary: Refinements as JSON Schema plus x-bynk-* extensions; the body-walked exact response set; a CLI and library surface with the spec as the product

**Context.** A `from http` service is an HTTP API, but its contract was
readable only through the toolchain: no way to generate a client, publish docs,
or diff the API in CI. The compiler already holds every fact an OpenAPI document
needs — the routes and params from the AST, each boundary type's shape and
refinements from the wire-contract IR (#855, which the emitter's codecs and the
editor's wire peek already share), and each handler's reachable `HttpResult`
statuses from the peek's body walk. A hand-maintained file drifts from the
handlers the moment either changes; one generated from those facts cannot.

**Decision A — refinements.** Every predicate a boundary type carries maps to a
JSON Schema keyword: `NonEmpty` → `minLength: 1`, `MinLength`/`MaxLength`/`Length`
→ `minLength`/`maxLength`, `Positive` → `exclusiveMinimum: 0`, `NonNegative` →
`minimum: 0`, `InRange(a, b)` → `minimum`/`maximum`, and `Matches(p)` → `pattern`
anchored as `^(?:p)$`, because the codec anchors the match and JSON Schema's
`pattern` searches. Cross-field predicates (`implies`) are agent invariants, not
wire types, so none reaches a boundary; had one, it would be carried as an
`x-bynk-*` key plus a description, which keeps the document valid and loses
nothing. Every refined schema carries the predicates as written in
`x-bynk-refinements` and spells them in its `description` in the codec's own
wording, so a consumer reading only the extension, or only the prose, sees the
whole check. An opaque scalar adds `x-bynk-opaque`; `Instant` and `Duration`
carry `x-bynk-base`.

**Decision B — responses.** A route's responses are exactly what its handler can
answer: the declared success only when the body constructs it, each `HttpResult`
variant the body constructs (grouped by status; a `Value` variant's body is `T`,
a message variant's is `{ "error": string }`, a redirect's a `Location` header),
and the `400` the boundary answers a malformed or refinement-violating request
with, described once as the `BynkBoundaryError` component. The set is the wire
peek's, so the document and the editor cannot disagree.

**Decision C — surface.** The primary surface is `bynk openapi [INPUT]
[--service NAME] [--out FILE] [--format json|yaml]`, which writes to stdout by
default so it composes in CI, and the library entry `bynk::openapi::document`.
The spec, not an editor view, is the deliverable; an editor export command is a
thin wrapper over the same path, and the in-editor service explorer and invoker
consume this output. The project must check clean: a document for a program that
does not compile would describe nothing.

**Consequences.** One document per run, with a tag per service; two services
answering the same method and path are refused, and `--service` picks one. A
type name declared differently in two contexts is qualified by its unit
(`orders.Money`). The standard `x-` namespace keeps the document valid 3.1. Not
in this increment: `--check` (diffing against a committed baseline to gate
breaking changes), the service explorer and invoker, security schemes from `by`
clauses, and the agent, queue and WebSocket boundaries.
