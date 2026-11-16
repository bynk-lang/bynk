---
level: minor
changelog: `bynk client`'s generated module carries a per-route and per-service contract hash (`API_CONTRACTS`) beside `WIRE_VERSION`, and `sendContract` sends the route's hash as `X-Bynk-Contract`; both informational (#3508 slice BC6)
probes: ts_writes
---

## ADR: bynk-client-http-contract-hash
title: `bynk client`: an informational contract hash for `from http` routes
summary: A generated client carries a FNV-1a hash of what it acts on for each route and service, sends it only on request, and nothing rejects a mismatch

**Context.** The proposal's Decision G gave a generated client a contract hash and a wire version to compare against a deployed Worker, and left server-side rejection out: `contract_hash` covers `on call` services only, and a hard refusal of a stale client is right between Bynk Workers deployed together and wrong for a public API whose browsers and cached bundles lag the server for days. This is slice BC6.

**Decision.**

- *The hash.* `bynk-check` gains `http_route_normal_form` and `http_route_contract_hash` beside `contract_hash`, over an `HttpRouteContract` the generator builds from what it already acts on: method, path, each parameter's kind, name and type (canonicalised by the same `canon_type` the `on call` contract uses, so a record's fields are sorted, predicates are a set and a generic is substituted), the success type and the replies as a sorted set of `status:variant:payload-kind`. It is FNV-1a, as `contract_hash`, and a skew detector rather than a security control. It moves when a client's behaviour would and not for a handler body, a doc comment, field order, CORS or a plain-JSON `Option` mode (the client reads both forms). A service's hash is the hash of its sorted route hashes.
- *The surface.* `export const API_CONTRACTS = { service: { service, routes: { fn: hash } } } as const`, keyed by the generated function names, beside the existing `WIRE_VERSION`. Each route constant also carries its hash.
- *Informational.* Nothing rejects a mismatch, on the client or the server: no `contract: enforce` policy, and no change to what a Worker serves. A stale client is `Unexpected`; the hash is for logs, `--check` and a deploy gate.
- *Wire version.* The HTTP JSON encoding follows wire version 1: both are read from the same wire model, so a change to it is a new wire version. `wire_format.rs` says so, and the hash is outside the version.
- *Sending it.* `ClientOptions.sendContract` (off by default) sends the route's hash as `X-Bynk-Contract`. Off by default because a custom header makes every cross-origin browser call preflighted, and the default `Access-Control-Allow-Headers` is `content-type` and, on a Bearer route, `authorization` (read in `workers_entry.rs`): the preflight would refuse every call unless the service's `cors { headers: [...] }` lists `x-bynk-contract` (an explicit list replaces the default). A server-side call has no preflight, so `viaBinding` returns `sendContract: true`, which a caller can override. The server ignores the header on a `from http` route; on `/_bynk/call/` the same header is enforced, which is a different route family.
- *Tested.* A hash is stable across runs and field order, unmoved by a handler body, moved by a field, a refinement or a new answer on the route that carries it and no other; the header is absent by default, equals `API_CONTRACTS` when sent, is ignored by every route, and is on for a Service Binding under `workerd`.

**Consequences.** A generated client names the contract it was made for. The text of existing routes gains one `contract` line each and the goldens are blessed. A project that never runs `bynk client` is unchanged.
