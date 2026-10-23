---
level: patch
changelog: "The first-party registry service takes publishes (R2, [#2336](https://github.com/bynk-lang/bynk/issues/2336)): `bynk publish` and `bynk yank` work against it, it signs with an online delegate it checks against `config.json`, and it passes the full `bynk registry conformance` run; `bynk-registry-core` now owns index lines' signing payload and rendering"
---

## ADR: registry-service-r2
title: The first-party registry's publish API
summary: The publish API as a Bynk service: shared admission and signing bytes through a wasm adapter, one PackageIndex agent per package, operator tokens, and a checked delegate key

**Context.** After R1 the hosted registry (`services/registry/`) serves the
protocol's read paths. Every package still has to be signed offline and seeded
by an operator. R2 adds the protocol's publish API, so that `bynk publish` and
`bynk yank` work against the hosted registry. Signing keys stay off publishers'
machines, and the server applies the same rules as the client and the reference
server (`bynk registry serve`). The accepted proposal is #2336, and the track is
`design/tracks/first-party-registry.md`. L4 (#2341) supplied the language
pieces: `headers`, `RawError` and `on rejected`.

**Decision.**

- **A. Admission is the shared crate, behind an adapter.**
  - `adapter registry.admission` binds `bynk-registry-core`'s C ABI, built to
    wasm and `includes`d. Its capability `Admit` provides:
    - `admit`;
    - `payload` (the canonical bytes a line's signature covers);
    - `render`;
    - `signer` (what `config.json` says about a key).
  - The module is built by `scripts/admission-module.sh` and git-ignored, never
    committed.
  - Refusals map to the reference server's statuses and `{error, message}`
    bodies.
- **B. The signed bytes come from the shared crate.**
  - `IndexLine`, its payload and its rendering, `Config` and the delegation
    payloads move into `bynk-registry-core`, which the `bynk` CLI now uses.
  - The ABI gains `call` with `line_payload`, `line_render` and `signer_info`,
    and the parity corpus covers them.
- **C. One `PackageIndex` agent per package is the single writer (D2, D10).**
  - It holds the package's lines and decides each append: a new line, a retry
    of the same tree (it returns the existing line, `200`), or another tree
    (`409 version_exists`). It also flips `yanked`.
  - The index file in the bucket is its projection, rewritten after each
    change. A failed write is repaired by the next one.
  - The archive is written first. It is content-addressed, so the write uses
    `onlyIf: Absent`.
  - A package seeded before the agent existed is adopted from its index file
    on the agent's first call.
- **D. Interim operator tokens in `Secrets`.**
  - `REGISTRY_TOKENS` is the reference server's token table as JSON:
    sha256, organisations and capabilities.
  - A request's bearer token, read through `headers.bearer()`, is hashed and
    looked up. An unknown or absent token is `401`; another organisation or
    capability is `403`.
  - R3 replaces the source.
- **E. A delegate from `Secrets`, checked before each use.**
  - The key is held in `REGISTRY_SIGNING_KEY` (PKCS#8) and
    `REGISTRY_SIGNING_PUBLIC`.
  - Before signing, the service confirms all of the following, and otherwise
    refuses with `500 internal`, naming the problem:
    - a delegation in the bucket's `config.json`, signed by the newest chain
      key, names the key, and that delegation verifies with `Crypto.verify`;
    - no revocation names the key;
    - a signature made with the key verifies.
  - The root key never reaches the service.
- **F. Proven end to end.** `scripts/check-publish.sh`, run in CI's
  `registry-service` job, does three things:
  - serves an empty registry with a delegate and two tokens, and passes all 23
    checks of the full `bynk registry conformance` run;
  - publishes, yanks, unyanks and resolves a package with the real client;
  - restarts the service with a key that isn't a delegate, and checks that it
    refuses to sign.

  Unit tests cover the agent's decisions, the token refusals, an admission
  refusal and the index file round trip.

**Deviations from the accepted proposal.**

- **The service signs; the agent only decides.**
  - A `SigningKey` can't be passed to an agent, and the agent needs no
    capabilities if it just keeps lines. So the handler admits, stores the
    archive, signs, then asks the agent to append.
  - A retry or a conflict wastes one signature. The agent still serialises
    every append, so D10 holds.
- **The index file is the codec's JSON, not the module's `render`.**
  - The service writes each line with `Json.encode`, which gives the same
    fields and values (a missing `toolchain` is `null`).
  - Clients parse lines, and `publish.read-back` and `consume.resolve` pass
    against it.
  - `render` stays in the ABI and the capability for a caller that needs the
    reference bytes.
- **Token lookup compares SHA-256 digests with `==`, not `Crypto.equal` per
  entry.**
  - Bynk has no way to fold an effect over a list, so one constant-time
    comparison per table entry can't be written yet.
  - The table holds digests, and the comparison is between digests of the
    presented token, so its timing says nothing usable about a token.
- **An agent is called from handlers, not from a capability provider.** A
  provider body that calls an agent emits a reference to the handler's `deps`
  outside its scope (a compiler bug, reported separately), so the yank path
  calls `PackageIndex` from its handlers.
- **PKCS#8 version 2 is normalised.** `bynk registry` writes ring's version-2
  Ed25519 document, and Web Crypto imports only version 1. The service rewrites
  version 2 as version 1 around the same seed before importing.
- **The router's `413` comes before authentication.** The cap is enforced
  before any handler runs, so an unauthenticated over-cap upload is `413` here
  but `401` from the reference server. Conformance sends its over-cap upload
  with a valid token, so it passes either way.
- **No `410 withdrawn` yet.** R5a.1 (#2340) hadn't merged when this was written,
  so the agent doesn't check a version's withdrawal. That joins R5a.2.

**Consequences.**

- The hosted registry takes publishes with the same rules, statuses and signed
  bytes as `bynk registry serve`.
- Accounts (R3), trusted publishing (R4), quotas and the reconciler (R5), and
  takedowns (R5a.2) build on this service.
- The `bynk` CLI's behaviour is unchanged.
