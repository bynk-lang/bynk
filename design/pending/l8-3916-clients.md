---
level: minor
probes: ts_writes
raises: ts_writes — the generated client declares a `Problem` type and decodes a `problem` reply body (#3916)
changelog: bynk openapi describes the errors of an errors { format: problem } route as application/problem+json, and bynk client types them as problem details
---

## ADR: problem-details-in-openapi-and-client
title: OpenAPI and the typed client follow a route's problem-details error format
summary: What bynk openapi and bynk client generate for a route that answers errors as RFC 9457 problem details

**Context.** The opt-in `errors { format: problem }` makes a route answer its error variants and router rejections as `application/problem+json`. A document or client that still described `{ "error": … }` and `{ "kind", "details" }` for such a route would be wrong about the very routes that opted in.

**Decision.** (A) `bynk openapi` documents, for a route whose effective error format is `problem`, each error variant with a message and the boundary's `StructuralMismatch`, `PayloadTooLarge` and `UnsupportedMediaType` answers as `application/problem+json` referencing one shared `BynkProblemDetails` schema (`type`, `title`, `status` required; `detail`, `kind`, `path`, `expected`, `actual` and `violation` optional). A route in `bynk` format is unchanged byte for byte, and the schema is added to `components` only when some route needs it. (B) `bynk client` gives such an answer a `problem: Problem` member in place of `error`, where `Problem` is the runtime's interface, decoded from one of two body kinds that need a string `type` and `title` and a numeric `status`: `problem` for an error variant (no `kind`) and `problem-rejection` for a router rejection (a string `kind`), so a handler's `BadRequest` and the boundary's `400` at one status are told apart. The contract hash moves for such a reply and only for it. (C) Nothing changes for a service that does not opt in.

**Consequences.** A TypeScript caller reads `res.problem.title` and `res.problem.detail` on an opted-in route and `res.error` on any other. The document and the client stay a faithful description of what the router answers. `Problem` joins the generated module's reserved surface names, so a boundary type called `Problem` is renamed.
