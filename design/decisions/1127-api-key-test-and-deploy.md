# 1127 — The `system` tier presents real API keys and the failure principals check the store's work; `bynk deploy` mints the pepper

- **Status:** Accepted (v0.836)

**Context.** Slice 1 of the API keys track (`design/tracks/api-keys.md`, spine #3939) ships in two parts. The first part (ADR block `api-key-mint`) carries the scheme, the seam, `mint` and the generated key store. A `system` case acting as an `ApiKey` actor then sends no key, and `bynk deploy` does not mint the pepper. This part closes both gaps, so the slice meets §14's "Done when" with live check K1.

**Decision A — the `system` tier's keys.** `by Client("acct-7")` mints a real key into the in-process key store, once per identity per harness, under the run's fixed test pepper, through the same `<Actor>KeyOwner.mint` that a handler's `Client.mint` reaches. The real seam then verifies it. Below `system`, the identity is given, as for every scheme.

**Decision B — the failure principals.** Each presents a key built from a real one:
- `by Malformed` sends a bad checksum, which the seam refuses before any lookup.
- `by Unknown` sends a key number its store never held, then a handle no store holds. Each is compared once against the dummy digest.
- `by Forged` sends the real id with a secret the store does not hold, which is compared once against the record.

Each principal's driver counts the key store's comparisons and fails the case unless the seam did exactly that work. The count wraps the platform primitive that the runtime's one constant-time comparison reaches on Node (`node:crypto`'s `timingSafeEqual`). So "the same work for an unknown key as for a wrong one" (D3 B) is pinned by the harness, not by timing. `by Malformed` and `by Unknown` on a route no `ApiKey` actor guards are `bynk.test.key_principal_needs_api_key_route`. `by Expired` on an `ApiKey` route is refused until `ttl` (slice 3), and `bynk.test.expired_needs_bearer_route`'s explanation now says so.

`Malformed` and `Unknown` join the reserved call-site principals, which `by <Name>` classifies before any actor lookup. So an actor may no longer take any reserved principal's name (`Nobody`, `Forged`, `Stale`, `Expired`, `Revoked`, `Malformed`, `Unknown` and the other bad-credential names): `bynk.actor.reserved_principal_name`. Such an actor could never be acted as in a test; before this, the clash was silent. This is a breaking change for a project that declares an actor with one of those names, and none in this repository does.

**Decision C — not against a real runtime.** `bynk test --against` refuses a case that needs the in-process key store (`bynk.test.remote_unsupported_api_key`):
- a case that acts as an `ApiKey` actor;
- a case that drives `by Malformed` or `by Unknown`;
- a case that drives `by Forged` and calls a route an `ApiKey` actor guards.

The harness would have to mint into the deployed key store, and the comparison counter needs Node. The callee map does not tie a principal to its call, so a case that also drives `by Forged` at another route is refused with it, which is the conservative answer.

**Decision D — the pepper is deployed like a session key, but `--force` keeps it.** An `ApiKey` actor's pepper variable joins `session_keys`, marked `"kind": "pepper"` in `bynk-secrets.json` so that `deploy` tells it from a signing key by kind, never by name. `bynk deploy` mints it as a Worker secret when it is unset (#4056, Decision B). Unlike a signing key, a set pepper is kept even under `--force`, and the deploy says so. The maintainer ruled this on #4595: a new pepper ends every key issued under the old one, rotation (`previous`) is refused until a later slice, and `--force` is also the flag that fixes a cookie-key mismatch. Replacing a pepper takes a deliberate act: a value the operator supplies, in the environment or `--secrets-file`, is always set, and it ends every earlier key. Live check K1 confirms this on a real account.

**Consequences.** A service author can test an `ApiKey` route end to end, failure cases included, and deploy it with no hand-made secret. Listing and revoking keys (slice 2), expiry, last use and rotation (slice 3), sums (slice 4), and OpenAPI and the client (slice 5) follow.
