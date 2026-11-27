---
level: minor
probes: emit_diagnostics, ast_importers, diagnostic_coverage, explained_codes
changelog: `bynk deploy` mints an `ApiKey` actor's pepper as a Worker secret, and a `system` test acts as an `ApiKey` actor with `by Client("…")` or refuses one with `by Malformed`, `by Unknown` and `by Forged`, each checking the key store did the same work (experimental, under `bynk.actor.ApiKey`)
---

## ADR: api-key-test-and-deploy
title: The `system` tier presents real API keys and the failure principals check the store's work; `bynk deploy` mints the pepper
summary: Slice 1's second part: the test principals for an `ApiKey` route, and pepper provisioning through the `SignedCookie` key path

**Context.** Slice 1 of the API keys track (`design/tracks/api-keys.md`, spine #3939) ships in two parts. The first part (ADR block `api-key-mint`) carries the scheme, the seam, `mint` and the generated key store. A `system` case acting as an `ApiKey` actor then sends no key, and `bynk deploy` does not mint the pepper. This part closes both gaps, so the slice meets §14's "Done when" with live check K1.

**Decision A — the `system` tier's keys.** `by Client("acct-7")` mints a real key into the in-process key store, once per identity per harness, under the run's fixed test pepper, through the same `<Actor>KeyOwner.mint` that a handler's `Client.mint` reaches. The real seam then verifies it. Below `system`, the identity is given, as for every scheme.

**Decision B — the failure principals.** Each presents a key built from a real one:
- `by Malformed` sends a bad checksum, which the seam refuses before any lookup.
- `by Unknown` sends a key number its store never held, then a handle no store holds. Each is compared once against the dummy digest.
- `by Forged` sends the real id with a secret the store does not hold, which is compared once against the record.

Each principal's driver counts the key store's comparisons and fails the case unless the seam did exactly that work. The count wraps the platform primitive that the runtime's one constant-time comparison reaches on Node (`node:crypto`'s `timingSafeEqual`). So "the same work for an unknown key as for a wrong one" (D3 B) is pinned by the harness, not by timing. `by Malformed` and `by Unknown` on a route no `ApiKey` actor guards are `bynk.test.key_principal_needs_api_key_route`. `by Expired` on an `ApiKey` route is refused until `ttl` (slice 3).

**Decision C — not against a real runtime.** `bynk test --against` refuses a case that acts as an `ApiKey` actor or drives `by Malformed` or `by Unknown` (`bynk.test.remote_unsupported_api_key`). The harness would have to mint into the deployed key store, and the comparison counter needs Node.

**Decision D — the pepper is deployed like a session key.** An `ApiKey` actor's pepper variable joins `session_keys`, so `bynk deploy` mints it as a Worker secret when it is unset and never replaces a set one (#4056, Decision B). A new pepper ends every key stored under the old one, so `--force` replacing it is the documented consequence. Live check K1 confirms this on a real account.

**Consequences.** A service author can test an `ApiKey` route end to end, failure cases included, and deploy it with no hand-made secret. Listing and revoking keys (slice 2), expiry, last use and rotation (slice 3), sums (slice 4), and OpenAPI and the client (slice 5) follow.
