---
level: minor
probes: emit_diagnostics, ast_importers, diagnostic_coverage, explained_codes
changelog: An `ApiKey` actor lists an identity's keys with `Actor.list(identity)`, revokes one with `Actor.revoke(identity, id)` and all of them with `Actor.revokeAll(identity)`, in a service handler or an agent's handler; a `system` test drives `by Revoked` at an `ApiKey` route, and `@rateLimit(key: identity)` counts a verified key holder (experimental, under `bynk.actor.ApiKey`)
---

## ADR: api-key-list-and-revoke
title: `list`, `revoke` and `revokeAll` on an `ApiKey` actor, legal in service and agent handlers; `by Revoked` and `key: identity` on its routes
summary: Slice 2 of the API keys track: the three members that read and end keys, their types and where they are legal, the generated store's new handlers, and the test principal and rate-limit key they unlock

**Context.** Slice 1 of the API keys track (`design/tracks/api-keys.md`, spine #3939) ships the `ApiKey` scheme, its seam, `mint` and the generated key store (ADR blocks `api-key-mint` and `api-key-test-and-deploy`). A key can be issued but not listed or ended. Slice 2 adds the members that do both (§9), `by Revoked` for the routes they protect (§10), and `@rateLimit(key: identity)` on those routes (§12).

**Decision A — the members.** On a local `ApiKey` actor:
- `Actor.list(identity)` is `Effect[List[ApiKeyInfo]]` and needs `given Clock`, as §9 sets, so that slice 3's expiry can prune without changing the signature. `ApiKeyInfo` is `{ id: ApiKeyId, label: ApiKeyLabel, created: Instant, expires: Option[Instant], lastUsed: Option[Instant] }`, in the `bynk` prelude under the same gate. It never holds a secret or a digest. `expires` and `lastUsed` are `None` until slice 3.
- `Actor.revoke(identity, id)` is `Effect[Bool]`. It answers `false`, changing nothing, when the key is not that identity's or is already gone. Revoking deletes the record, so the seam then refuses the key exactly as it refuses one never minted: one compare against the dummy digest.
- `Actor.revokeAll(identity)` is `Effect[Int]`, the number of keys revoked.

**Decision B — where they are legal.** They need no pepper, only calls to the generated agents, so they are legal wherever an agent can be called: a service handler or an agent's handler (an account-closure workflow can call `revokeAll`). A provider or a function cannot reach an agent, so there they are `bynk.actor.api_key_member_in_provider`. `mint` keeps its own rule (`bynk.actor.api_key_mint_outside_handler`). An agent whose handlers call one of the three now rebuilds its `env` inside its Durable Object, as it does for any agent call. Before this, such an agent would have reached an in-process key store that is not the Worker's.

**Decision C — the store's new handlers.** `<Actor>Keys(handle)` gains:
- `labelsOf` and `createdOf`, the two maps `list` reads;
- `revoke(keyNo)`, which removes the key number from every map, and refuses on a retired store;
- `clear()`, which empties every map and retires the store in one commit, and answers the count.

`<Actor>KeyOwner(identity)` gains:
- `current()`, the identity's handle;
- `revoke(handle, keyNo)`, which acts only when the handle is the identity's own, so another identity's key id is refused there;
- `revokeAll(fresh)`, which clears the current store and then takes `fresh` as the identity's handle (§6).

`fresh` is drawn by the runtime's random handle generator, the one `mint` uses, and is never derived from the identity. If that second commit fails, the owner still names a retired store, so the next `mint` takes a fresh handle first, as slice 1's `mint` already does on a retired store.

`revoke` answers `false` the same way for an id that is not a key id of the actor (the runtime parses it first), another identity's key, a key number the store never held, and a retired store. So a caller cannot tell them apart, and cannot probe which keys exist. A test pins the four answers byte for byte, and another pins `ApiKeyInfo`'s fields, so `list` cannot start to carry key material unnoticed.

**Decision D — `by Revoked` on an `ApiKey` route.** `by Revoked`, until now legal only on a `sessions = true` `SignedCookie` route, is admitted on a route a single `ApiKey` actor guards. The driver mints a real key for an identity of its own (`bynk-test-revoked`), revokes it through `<Actor>KeyOwner.revoke`, and presents it. It counts the store's comparisons like the other key principals, and fails the case unless the seam compared once against the dummy digest. `bynk.test.revoked_needs_sessions_route` keeps its code, since codes are stable, and its message names both routes it is legal on. `bynk test --against` refuses `by Revoked` at an `ApiKey` route (`bynk.test.remote_unsupported_api_key`), as it refuses `by Forged` there.

**Decision E — `@rateLimit(key: identity)`.** `key: identity` admits a single `ApiKey` actor, through the same verify/run split as the other schemes (ADR 1023 B, F). The verify step runs the seam's statements, the key store's lookup included. The limiter then counts the verified holder as `identity:<Actor>:<sub>`. A key that fails verification gets its `401` and never reaches the limiter. So `key: identity` bounds a verified caller, not a guessing attacker, and such a route still warns `bynk.actor.api_key_unlimited`. A route carries at most one `@rateLimit`, so the `clientIp` limit that would silence the warning cannot sit on the same route; it belongs at the edge. Allowing a pre-authentication `clientIp` limit beside an `identity` limit on one route waits on a maintainer decision (#4614).

**Decision F — the workers entry validates a prelude-typed path parameter with its codec.** `DELETE("/keys/:id") (id: ApiKeyId)` is §9's own example. The Workers entry validated a path parameter of a named type as `handlers.<T>.of`, which a `bynk` prelude type does not have. It now uses the codec `handlers` re-exports, as it already did for a type taken from a consumed context.

**Consequences.** A service can list and end its clients' keys, and an agent can end them in a workflow. Expiry, last use and rotation (slice 3), sums (slice 4), and OpenAPI and the client (slice 5) follow.
