---
level: minor
probes: emit_diagnostics, ast_importers, diagnostic_coverage, explained_codes
changelog: An `ApiKey` actor's keys expire under its `ttl`, record their last use at most once an hour, and survive a pepper rotation through `previous`, re-hashed on their next use; `Actor.sweep(identity)` reports the keys a rotation would still end, `list` shows each key's pepper, and a `system` test drives `by Expired` at an `ApiKey` route (experimental, under `bynk.actor.ApiKey`)
---

## ADR: api-key-expiry-and-rotation
title: An `ApiKey` actor's `ttl` and `previous`, last use, `sweep`, and a key check that never writes on a refusal
summary: Slice 3 of the API keys track: key expiry, last-use recording, pepper rotation with lazy re-hashing, the per-identity sweep, the `pepper` field of `ApiKeyInfo`, `by Expired` on an `ApiKey` route, and the plan note for a pepper that cannot be rotated

**Context.** Slices 1 and 2 of the API keys track (`design/tracks/api-keys.md`, spine #3939) mint, verify, list and revoke keys (ADR blocks `api-key-mint`, `api-key-test-and-deploy`, `api-key-list-and-revoke`). They refuse `ttl` and `previous`, so a key never expires and the pepper cannot change without ending every key. Slice 3 adds both, the last-use record (§16 Q7) and the sweep of §7, with live check K2.

**Decision A — `ttl`.** An `ApiKey` actor may declare `ttl`, a positive whole number of seconds (`90.days`). The scheme-config rule that admits a duration only for `SignedCookie` admits it for this one key.
- `mint` sets `expires` to the creation time plus the `ttl`, and `ApiKeySecret.expires` reports it.
- The seam compares the presented key first and checks expiry after, so an expired key gets the same `401` after the same work as a forged one.
- `mint` (before the 100-key cap) and `list` remove expired keys.
- `by Expired` is admitted on a route a single `ApiKey` actor with a `ttl` guards. The driver mints a real key the `ttl` and an hour before the harness clock and expects one compare against the record. On an actor without a `ttl` it stays `bynk.test.expired_needs_bearer_route`.
- Expiry and last use are stamped by `mint` with the program's `Clock`, so the seam reads the same clock: the harness clock at `system`, unlike a JWT `exp`.

**Decision B — last use.** An admitted key's last use is recorded when it has none or it is an hour old (§16 Q7). `ApiKeyInfo.lastUsed` reports it.

**Decision C — the key check never writes on a refusal.** An agent handler that may write commits its state on every call, whichever branch it takes, so a `verify` that recorded last use would write on every refusal too. So:
- `<Actor>Keys.verify` only reads. It answers `Some(<Actor>KeyCheck { identity, touch })` for an admitted key, else `None`.
- The seam calls `<Actor>Keys.touch` only when `touch` is true. `touch` records last use, re-hashes a key admitted under `previous`, and notes the `kid`s now live. That is at most once an hour per key, plus once after a rotation, and never on a refusal.
- `touch` re-checks that the record still exists, so a key revoked in between is not brought back.
- `<Actor>KeyCheck` is a generated record, reserved like the two agents (`bynk.actor.api_key_agent_name_reserved`).
- This departs from §7's "re-hashed under `secret` in the same commit": the re-hash is the next commit, made by the same request before it answers. If `touch` faults, the request faults as any key-store fault does (never an admission, never a `401`), and the key, still under `previous`, is re-hashed on its next use.

**Decision D — rotation.** An `ApiKey` actor may declare `previous`, the variable holding the pepper being rotated out, read for verification only and optional at run time.
- While it is set, the seam hashes a presented key under both peppers, and the store compares the hash its record's `kid` names.
- A key admitted under `previous` is re-hashed under `secret` (Decision C).
- A record under neither `kid` is compared against the dummy hash and refused with the ordinary `401`, after the same work and with no write.
- `mint` and `sweep` remove such records. `list` shows them.
- Dropping `previous` ends every key not used since the rotation began, as §7 rules, and the reference says so beside the rotation steps.

**Decision E — `sweep` and `pepper`.** These follow the coordinator's ruling on the slice 3 plan.
- **`Actor.sweep(identity)`** is `Effect[Int]` and needs `given Clock`. It removes the identity's expired keys and keys under neither pepper, and answers how many remain under `previous`: the keys dropping it now would end. A service sums it over its own identities, since no store-wide index exists (Q1).
- **Where `sweep` is legal.** It tells the live pepper's keys from the rest, which takes the pepper, so like `mint` it is legal only in a service handler (`bynk.actor.api_key_mint_outside_handler`, whose message now names both).
- **`ApiKeyInfo.pepper`** is `ApiKeyPepper` (`Current | Previous | Retired`), in the `bynk` prelude under the same gate (Q2).
  - In a service handler, `list` reads it against the live peppers.
  - In an agent's handler, which holds no pepper, it reads against the `kid`s the store last saw, at its last mint, sweep or admitted key. During a rotation that view can lag until the identity's next request.

**Decision F — a minted pepper cannot be rotated.** Cloudflare never returns a secret, so nobody holds the value of a pepper `bynk deploy` minted, and it cannot be moved into `previous`.
- The reference says to supply your own pepper from the first deploy to keep the option to rotate.
- `bynk-secrets.json` lists a pepper's `previous` beside it (additive within version 2).
- When the actor declares `previous` and the pepper is managed, the plan prints `note CLIENT_KEY_PEPPER: rotation needs a supplied pepper; …`, and the JSON plan carries `rotation_needs_supplied_pepper`.
- `previous` is never minted: it is set only when supplied.

**Consequences.** A service can issue keys that expire, see which keys are stale, and rotate its pepper without ending the keys in use. Live check K2 confirms rotation on a real account. Sums (slice 4), and OpenAPI and the client (slice 5) follow.
