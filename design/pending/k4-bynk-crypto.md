---
level: minor
probes: emit_diagnostics, diagnostic_coverage
changelog: `bynk.crypto` (experimental) — effectful Web Crypto digests, HMAC, constant-time comparison and sign/verify with non-storable key handles, and `Random.bytes(n)` (standard-library K4)
---

## ADR: bynk-crypto-effectful-web-crypto
title: `bynk.crypto`: effectful Web Crypto with non-storable key handles
summary: One `Crypto` capability in a flat sibling unit, every operation an `Effect` over `crypto.subtle`, keys as opaque non-storable handles, constant-time comparison, a binding per platform

**Context.** A Bynk program could not hash, sign or verify anything: the only
crypto was inside the runtime's own verifiers (`runtime/src/auth.ts`), which a
program cannot reach. `Bytes.compare` is documented as not constant-time, and
there was nothing to compare secrets with. The first-party registry (#2308)
needs SHA-256 of publish tokens compared in constant time, Ed25519 signing of
index lines with a key from `Secrets`, and random tokens. Standard-library track
D5 settles that crypto is effectful over Web Crypto, D6 that a new capability is
a flat sibling unit, and R-16 that anything handling keys, signatures, secrets
or untrusted bytes gets a security review.

**Decision.**

- **A: one capability, a flat sibling unit.** `bynk.crypto` exports one
  capability, `Crypto`, and is consumed as `consumes bynk.crypto { Crypto }`.
  ADR 0012's closed `bynk` list is unchanged and every dependency on crypto is a
  greppable line. `given Crypto` reads as "this handler does cryptography". The
  unit is Tier 0, platform-free, at `id_block` 7, and ships experimental
  (`[unstable] allow = ["bynk.crypto"]`) until the first-party registry has
  signed with it in production (#2308 R6). `Digest`, `Signature` and
  `CryptoError` are extensible.
- **B: effectful, over `crypto.subtle` only.** Every operation returns `Effect`
  because `crypto.subtle` is asynchronous everywhere. The cost, stated in the
  docs: no hashing in pure functions, refinements or invariants. There is no
  third-party crypto and no polyfill, so a platform without `crypto.subtle`
  (a browser outside a secure context) fails loudly (R14.5). The algorithms are
  the intersection all three platforms support: SHA-256/384/512, HMAC over
  them, Ed25519, ECDSA P-256 with SHA-256, and RSASSA-PKCS1-v1_5 with SHA-256.
- **C: keys are opaque handles that cannot be stored or sent.** `SigningKey`
  and `VerifyKey` are declared `opaque` in the unit and hold a non-extractable
  `CryptoKey`; `.raw` is confined to the defining unit, so the key bytes cannot
  be read back. Like `Stream`, they are non-storable and non-boundary: a
  `store`, a record or event field, a handler parameter or result, or a wire
  type naming one is `bynk.types.key_at_boundary`. The check finds the types by
  the unit's reserved span file, not by name, so a user type called
  `SigningKey` is not mistaken for one. Passing key bytes to every call was
  rejected: the key would then be an ordinary `Bytes` that can be stored,
  returned or logged. A storable handle was rejected: a non-extractable
  `CryptoKey` cannot be serialised.
- **D: constant time where it matters, and said so.** `verifyHmac` and `verify`
  use `crypto.subtle.verify`. `equal(a, b)` compares secrets and digests in
  constant time for equal lengths (the length is not secret): the platform
  primitive where one exists (`crypto.subtle.timingSafeEqual` on Workers,
  `node:crypto`'s `timingSafeEqual` on Node), else the double-HMAC comparison —
  HMAC both values under a fresh random key and compare with `subtle.verify` —
  which is the browser's path. `Bytes.compare` is unchanged and its docs point
  at `Crypto.equal`. The docs list the timing guarantee per operation.
- **F: a binding per platform, run for real in tests.**
  `Binding::PerPlatformFiles` gives a platform-free unit one binding file per
  platform; `Binding::PerPlatform` stays the `bynk` surface's. The three
  bindings (`crypto-{cloudflare,node,browser}.binding.ts`) are one body with one
  marked region, the comparison primitive, and a drift test keeps them so. The
  unit-tier test double for `Crypto` is the binding itself: crypto is
  deterministic for given inputs, and a fake would test nothing. `stub` and
  `expect` still work. Each binding has a behavioural test per method (R-13)
  with known-answer vectors (FIPS 180-4, RFC 4231, RFC 8032) and
  cross-checks against `node:crypto` in both directions.
- **Review (R-16).** The implementing PR carries the checklist: keys are
  non-extractable; no key material appears in errors or logs (every failure
  reason is a fixed string); the constant-time paths are as documented; no
  third-party code; algorithm parameters are fixed by the enums, never
  caller-chosen; the browser secure-context failure is loud.

**Consequences.** A Bynk program can hash, authenticate and sign, and a reviewer
sees it in `given Crypto`. Because the unit is experimental it may change in any
release until the registry has used it. Out of scope: encryption and decryption,
key generation and export, key derivation, JWT and JOSE helpers, X.509, and pure
hashing (D5 rules it out). A first-party unit's key types are now a kind of
non-boundary value that the checker knows by origin; a later unit with a similar
handle can reuse the same mechanism.

## ADR: random-bytes
title: `Random.bytes`
summary: `Random` gains `bytes(n)` over `crypto.getRandomValues`, bounded 1 to 65,536; `Random.int` is labelled not for secrets

**Context.** `Random` offered only `uuid()` and `int(lo, hi)`, and `int` draws
from `Math.random()`. There was no way to get random bytes of real entropy for a
token, a nonce or a key.

**Decision.** `Random` gains `bytes(n: Int) -> Effect[Bytes]` over
`crypto.getRandomValues`, on all three platforms. `n` must be between 1 and
65,536 (Web Crypto's per-call limit); anything else fails the effect with an
error naming the bound, rather than clamping or looping. It stays on `Random`,
not `Crypto` (D6): it is the same capability family as `uuid()` and needs no new
`consumes`. `Random.int` is unchanged, and its docs now say it is not for
secrets and point at `Random.bytes`. Under test the `Random` double draws bytes
from the same fixed-seed generator as `int`, so a test is deterministic.

**Consequences.** Every emitted `bynk` surface binding and the `Random`
interface gain one operation; existing programs are unaffected. A program that
needs a secret now has the right primitive, and a reviewer can see `Random.bytes`
rather than `Random.int` at the call.
