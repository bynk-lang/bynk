---
level: minor
probes: diagnostic_coverage
changelog: First-party registry R5b — registry key delegation and revocation. A registry's chain key can delegate line signing to another key and later revoke it (`delegations` and `revoked` in `config.json`, which then speaks protocol 2), and a line's signature may be re-signed. Clients accept lines signed by the chain or a valid, unrevoked delegate, refuse a revoked delegate's lines naming the revocation, and ignore (and report) a delegation or revocation no chain key signed. A leaked delegate is replaced without clients re-pinning. New `bynk registry rotate`, `delegate`, `revoke` and `resign`; `serve` and `add` sign with a delegate; `conformance` gains `config.delegations`, `publish.signer` and `read.revoked`
---

## ADR: registry-key-delegation-and-revocation
title: A registry's chain key may delegate line signing and revoke a delegate; lines may be re-signed
summary: First-party registry track R5b, amending ADR 0564's rotation-only trust and ADR 0563's "only `yanked` may change" — `delegations` and `revoked` signed by chain keys (protocol 2); a line verifies under a reached chain key or a valid, unrevoked delegate; `sig` may be replaced by another valid signature over the same payload; `rotate`, `delegate`, `revoke`, `resign`

**Context.** The key chain is linear and a client accepts every key it reaches from its anchor (ADR 0564), so no
key can be revoked, and whoever holds a leaked key can append links. For the first-party registry, whose anchor
ships in every `bynk`, any leak of its online key would need a toolchain release. The track settled on an offline
root with revocable delegated signing keys (#2308 §6, D5, Q5 re-settled after review). Proposal #2316, decisions
A–F.

**Decision.**

- **Delegations and revocations (A).** `config.json` may list `delegations` (`{key, sig}`, `sig` a chain key's
  signature over `{"delegate","registry"}`) and `revoked` (`{key, sig}` over `{"registry","revoke"}`). "A chain
  key" is any key the trusted chain reaches. A delegate signs index lines only: it can't extend the chain,
  delegate or revoke. Only delegates can be revoked; a leaked chain key still means re-pinning. Expiring
  delegations and a branching chain were rejected.
- **Verification (B).** A line verifies when a reached chain key, or a delegate whose delegation verifies and
  which no verifying revocation names, signed its payload. A revoked delegate's line is
  `bynk.registry.bad_signature`, naming the revocation. A delegation or revocation whose own signature doesn't
  verify is ignored, with a notice. The lock is unchanged: it records the chain anchor.
- **Re-signing (C).** A line's `sig` may be replaced by another valid signature over the same payload, so nothing
  but the signature changes. Recovery is: delegate a new key, publish; re-sign every line; revoke the leaked key,
  publish — no line is ever unverifiable.
- **Protocol 2 (D).** A config with `delegations` or `revoked` must say `"protocol": 2`, and is written so; one
  without them stays `1`, byte-identical. This toolchain reads both; an older one stops on `2` with its clear
  version message rather than refusing delegate-signed lines with no hint.
- **Tooling (E).** `bynk registry rotate` (a chain link, replacing the hand edit), `delegate`, `revoke` (refusing
  to leave a line with no valid signer unless `--force`) and `resign` (every line, through a rename). Each works
  on any directory holding a `config.json`, so a chain key can stay offline. `serve` and `add` sign with the
  newest chain key or a valid, unrevoked delegate, and refuse a revoked one.
- **Conformance and the boundary (F).** `bynk registry conformance` gains `config.delegations`,
  `publish.signer` and (with `--read-only`) `read.revoked`. Freshness — a stale `config.json` hiding a
  revocation — isn't covered; that is the TUF depth packaging deferred, and the protocol page says so.

**Consequences.** A registry that signs with a delegate recovers from a leaked delegate without clients changing
anything; registries that never delegate are unchanged. The first-party registry pins its offline root in
`FIRST_PARTY` (R6) and signs with a delegate.
