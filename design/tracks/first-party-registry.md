# The first-party registry — a hosted Bynk service, written in Bynk

- **Status:** Draft (settling). Spine: to be opened. This track is packaging
  slice 4b ([#843](https://github.com/bynk-lang/bynk/issues/843),
  [`packaging.md`](packaging.md) §9). The packaging track deferred the
  service's implementation to "slice 4b's own track" (packaging §11 Q9) and
  fixed only the protocol it must speak.
- **Realises:**
  - packaging §6.1 ("the first-party registry is one deployment of this
    protocol, with an `api`, org ownership, and its own account policy");
  - packaging §6.2's claim-blocking and publication rule;
  - the `FIRST_PARTY` constant in `bynk-project/src/lock.rs`, which is
    `None` until this track lands.
- **Posture:** Feature track per
  [ADR 0076](../decisions/0076-feature-track-posture.md). It qualifies on all
  three axes (§1.2).
- **Governing records:**
  - [ADR 0563](../decisions/0563-the-registry-protocol.md): the protocol.
  - [ADR 0564](../decisions/0564-registry-trust-and-routing.md): trust and
    routing.
  - [ADR 0565](../decisions/0565-registry-version-selection.md): version
    selection.
  - [ADR 0570](../decisions/0570-registry-publish-api.md): the publish API.
  - [ADR 0571](../decisions/0571-reference-registry-server.md): the reference
    server.
  - [ADR 0575](../decisions/0575-adapter-npm-approval.md): adapter npm
    approval.
  - [ADR 0023](../decisions/0023-platform-adapter-before-stdlib.md): platform
    and stdlib work never share an increment.
  - [ADR 0109](../decisions/0109-handler-atomic-commit.md): handler atomic
    commit.
  - The standard-library track ([`standard-library.md`](standard-library.md),
    #1936). It owns every `bynk-std` capability this service needs (§4).

## 1. The theme

### 1.1 What this track settles

Bynk runs the first-party instance of the registry protocol: the registry
every organisation not routed elsewhere resolves on. The track settles:

1. **What the service is.** Its contexts, its storage, how it signs, and how
   it serialises writes (§3).
2. **What it needs from the language and `bynk-std`** that doesn't exist yet,
   and which track owns each gap (§4).
3. **Accounts, organisations and tokens.** Who may publish under which
   organisation, how a name is claimed or claim-blocked, and how CI publishes
   (§5).
4. **Operating it.** Key custody, deploys, backups, abuse handling and
   takedowns (§6).
5. **Proving it speaks the protocol.** A conformance suite that any registry
   can be run against: this service, the reference server and private ones
   (§7).
6. **Going live.** Setting `FIRST_PARTY`, enforcing the publication rule, and
   the docs (§10, R6).

**It is written in Bynk.** It is the largest Bynk program anyone will have
run in production, so it is also the language's most demanding dogfood. The
capabilities it lacks are built as ordinary `bynk-std` and language
increments, each landing with its first real consumer.

### 1.2 Why a track (the ADR 0076 trigger)

- **Many increments.** Prerequisites span two other tracks, then the service
  slices and go-live.
- **The surface isn't settled.** Accounts, organisation claiming, trusted
  publishing and takedown policy are all open (§11).
- **A security boundary.** It holds the signing key whose public half ships in
  every `bynk` binary. A compromise means a toolchain release (packaging
  §6.2).

### 1.3 In this repo now, extracted later

The service lives in this repository until the protocol and the `bynk-std`
surface it needs have settled, and is then extracted to its own repository.
While it is here:
- a capability it needs ships in the same PR as its first real use;
- a protocol change updates the client, spec, reference server and service in
  one review;
- the repo's CI catches a compiler change that would break a real service
  before it merges.

These rules keep extraction cheap:

- **E1. A self-contained Bynk project** at `services/registry/`, with its own
  `bynk.toml`. It reaches only `bynk-std` and its own files, never a relative
  path into the compiler's crates. Extraction is then a `git filter-repo` plus
  a pinned toolchain.
- **E2. No Rust inside the service.** Rust logic it shares (§3.4) crosses an
  adapter boundary, as if it were an external package. On extraction it
  becomes one.
- **E3. No account-specific configuration in git.** Cloudflare account IDs,
  secrets and the signing key are supplied at deploy through `Secrets` and CI
  secrets.
- **E4. Its own CI job.** `bynk check`, `bynk test`, and the conformance
  suite (§7) against a local `bynk dev`, on every PR. It deploys only from a
  separate, manually triggered workflow.
- **E5. It never depends on itself.** No registry dependencies, only path and
  git ones. A broken registry, or a bad toolchain release, can then never stop
  the fix from being redeployed.

**When to extract.** Once R6 has shipped and two consecutive `bynk-std`
releases have needed no change for the service. Extraction is its own slice
(R7).

## 2. What's already true

- **The protocol and its client are complete.**
  - The protocol covers `config.json`, signed index lines, `bynk-tree-v1`
    archives and the publish API; `registry-protocol.md` specifies it.
  - The `bynk` client resolves, verifies, fetches, publishes
    (`bynk publish`), yanks (`bynk yank`) and approves (`bynk approve`).
  - The `organisation → registry` routing falls back to the registry named
    `bynk`.
- **The first-party registry isn't live yet.** `FIRST_PARTY` is `None`, so an
  unrouted organisation is `bynk.registry.unavailable`. A mirror of `bynk` is
  refused for the same reason. Going live is setting that constant (an index
  URL and a pinned `ed25519:` key) in a toolchain release.
- **A reference server exists.** `bynk registry serve` (ADR 0571) is
  single-threaded, uses tiny_http, and runs over a directory with hashed
  tokens. It is the behavioural model for this service's publish path, and
  `commands::admit` holds the admission rules both must apply.
- **`bynk-std` (surveyed 2026-10-22)** has:
  - `Bytes` with concat, slice, compare, and hex and base64url conversion (K1,
    #2016);
  - `Kv` with String values only (derived instances, S4);
  - `Secrets`, and `Random` (`uuid` and `int` only);
  - no `Crypto` (K4, planned), no R2 (P5, planned), no `Bytes` Fetch bodies
    (K5, planned).
- **HTTP services** can:
  - **answer with `Raw(body, contentType)`,** a `Bytes` body under any
    content type. That's enough to serve index files and archives.
  - **use automatic ETags and `304`s.**
  - **authenticate with a `Bearer` actor (HS256 JWT) or an `Oidc` actor**
    (RS256/ES256 against a JWKS).

  But a request body is **JSON only**: a `body` parameter is parsed from JSON
  before the handler runs. The publish API's `PUT` takes a raw
  `application/vnd.bynk.tree-v1` body, so that's a language gap (§4).
- **Agents serialise per key.** An agent instance runs one handler at a time,
  and its state commits atomically with the handler (ADR 0109). That is
  exactly the single-writer-per-index discipline the reference server gets
  from being single-threaded.

## 3. The service

### 3.1 Shape

```text
services/registry/
  bynk.toml                 # [project] name = "registry"; no registry dependencies (E5)
  src/
    read.bynk               # context: GET config.json, index files, archives
    publish.bynk            # context: the publish API (PUT, POST yank/unyank)
    index.bynk              # agent PackageIndex, keyed by org/package: the single writer
    accounts.bynk           # context: users, organisations, claims, tokens
    admission.bynk          # adapter over the shared admission module (§3.4)
    signing.bynk            # commons over bynk.crypto: canonical payload, sign
  tests/ …
```

### 3.2 The read path

Every read is a `GET` of a static-shaped path, served from storage with
ETags. Nothing in the read path signs or decides anything, so it can be cached
at the edge without risk:

- **`config.json`:** `protocol`, `id = "bynk"`, `dl`, `auth = "none"`, `api`,
  and the key chain.
- **`<org>/<package>`:** the index lines, as the single writer last wrote
  them.
- **`dl/sha256/<hex>`:** the archives, which are immutable.

### 3.3 The write path

1. `publish` authenticates the token and authorises the organisation and the
   capability (§5).
2. It hands the upload to `admission`, which unpacks and validates it by
   `commands::admit`'s rules and returns the line's fields and the canonical
   tree checksum, or a refusal.
3. It calls the package's `PackageIndex` agent. The agent:
   - refuses an existing version (`409`);
   - stores the archive at `dl/sha256/<hex>`;
   - signs the line;
   - appends it;
   - republishes the index file.

   That order means a line never exists without its archive.
4. The response is `201` with the line, exactly as ADR 0570 specifies.

Yank and unyank go to the same agent and flip `yanked`, re-signing nothing.

### 3.4 Admission: shared, not reimplemented

The most dangerous code to duplicate is the hardened archive reader and the
manifest check: traversal, ordering, size caps, the canonical checksum, and
the `[package]` and dependency rules. Reimplementing it in Bynk would mean:
- octet-level `Bytes` access (a K1 follow-on);
- a TOML parser in Bynk;
- a second implementation that can drift from the client's.

**Proposed (D3):** compile the Rust admission path (`archive::unpack` plus the
`admit` checks, refactored so they don't touch the filesystem) to
WebAssembly. The `bynk-wasm` crate's toolchain already does this. A
`bynk-registry-admission` module, wrapped by an `adapter admission` whose
binding loads it, exposes one operation: `admit(upload: Bytes, name: String,
version: String) -> Result[Admitted, Refusal]`. The client, the reference
server and the service then apply the same rules from the same source. E2
holds, because the service sees only an adapter.

## 4. Prerequisites: what Bynk needs first

Each gap is built by the track that owns it, as an ordinary increment, with
this service as its first consumer. Stdlib and platform work never share an
increment (ADR 0023).

| Gap | Needed for | Owner | Slice |
|---|---|---|---|
| Raw request bodies: a handler parameter typed `Bytes`, with its content type and a size cap | `PUT` of an archive | Language (HTTP surface) | new, proposed here (L1) |
| SHA-256 digests, Ed25519 sign and verify, key import, constant-time compare | signing lines; hashing tokens | `bynk.crypto` | stdlib K4 |
| `Random.bytes` | minting tokens | `bynk.crypto` / `Random` | stdlib K4 |
| R2: `Bytes` objects, streams, conditional put | archives and index files | `bynk.cloudflare` | stdlib P5 |
| Byte streams | archives up to 64 MiB without buffering twice | L0 | stdlib P5 prerequisite |
| Rate limiting | abuse handling | `bynk.cloudflare` | stdlib P3 |
| A wasm module behind an adapter binding | shared admission (§3.4) | Emit / adapters | new, proposed here (L2) |

L1 and L2 are this track's own language slices. The rest are already on the
standard-library track's slice list (#1936). This track only asks for them to
be ordered earlier there (§12), and adds nothing to that track's scope.

## 5. Accounts, organisations and tokens

**Proposed (D4):**

- **People sign in with an OIDC provider** (GitHub first), through the `Oidc`
  actor. The service keeps no passwords.
- **An organisation** is claimed by a signed-in user, who becomes its owner.
  Owners add members, who may publish. Organisation names follow the manifest
  rule (`[a-z][a-z0-9-]*`).
- **Reserved names.** `bynk` is reserved for the toolchain (packaging §12).
  Names an operator reserves for impersonation risk are refused.
- **Claim-blocking** (packaging §6.2). A private organisation may reserve its
  name without publishing, so an unrouted `acme` can never resolve to a
  stranger's `acme`. A blocked name serves no index, and publishes to it are
  refused.
- **Tokens** are opaque, random (`Random.bytes`), stored as SHA-256 digests,
  and scoped to organisations and to `publish` or `yank`, exactly as in the
  reference server (ADR 0571 E). Unlike there, they also expire and can be
  revoked.
- **Trusted publishing.** A CI job publishes with its OIDC token (for example
  GitHub Actions'), exchanged for a short-lived publish token. The token is
  bound to the repository and workflow that the organisation configured. No
  long-lived secret sits in CI.
- **The publication rule** (packaging §6.2). Admission refuses a package
  whose dependencies would resolve outside the first-party registry, meaning
  any dependency whose organisation isn't a first-party organisation.

The `bynk` client needs no change for any of this. It sends a bearer token,
and how the token was issued is the service's business.

## 6. Operating it

- **Key custody (D5).**
  - The signing key lives only in the service's `Secrets`, imported at deploy.
  - The **root** of its rotation chain is kept offline, and only its public
    key ships in `FIRST_PARTY`. The online signing key is the next link,
    signed by the root.
  - So a leaked online key is replaced by a planned rotation: the root signs a
    new link, and clients follow the chain (ADR 0564). Only a leaked root
    needs a toolchain release.
  - This is packaging §6.2's rotation, used as designed.
- **Deploys** come only from a manual workflow with an approval gate (E4).
  Each deploy runs the conformance suite against staging first.
- **Durability.** Archives and index files are in R2. Each index line is also
  kept in the agent's own state, so an index file can always be rebuilt from
  its writer. There's a scheduled export of both.
- **Abuse.**
  - Upload size caps (64 MiB, as the reference server).
  - Per-token and per-IP rate limits (P3).
  - A per-organisation storage quota.
- **Takedowns.** Yanking never deletes (packaging §12), but legal takedowns
  and malware will happen. The open question is Q4.

## 7. Conformance

**Proposed (D6):** a conformance suite in this repository (Rust, beside the
registry tests) that takes a registry URL and a token. It runs the protocol's
observable behaviour against it:
- `config.json`'s shape and key chain;
- a publish, read back and verified;
- `409` on a republish;
- `401` and `403` on bad tokens;
- `400` on a mismatched or traversing upload;
- `413` past the cap;
- yank and unyank;
- the redirect rule;
- a consumer resolving and locking what was published.

The reference server passes it in CI. The service passes it against
`bynk dev` on every PR, and against staging before every deploy. A private
registry operator can run it too.

## 8. Self-hosting constraints

- **E5 again.** No registry dependencies, ever, so the service can always be
  built.
- **Toolchain.** In this repo it builds with the in-tree toolchain (that's the
  dogfood). Its deploy workflow builds with the **latest released**
  toolchain, so production never runs unreleased compiler output.
- **The registry isn't on the fetch path of its own build,** and `bynk`'s
  read path doesn't depend on any of the service's write-side state. A
  write-side outage leaves resolution working.

## 9. Decisions (proposed)

| # | Decision | § |
|---|---|---|
| D1 | The service is written in Bynk, in this repo at `services/registry/`, under extraction rules E1–E5, and extracted after go-live | §1.3 |
| D2 | A `PackageIndex` agent per package is the single writer; an archive is stored before its line | §3.3 |
| D3 | Admission is the Rust `admit` path compiled to wasm behind an adapter, not a reimplementation | §3.4 |
| D4 | OIDC sign-in, owner-and-member organisations, claim-blocking, hashed scoped expiring tokens, trusted publishing from CI | §5 |
| D5 | An offline root key pinned in `FIRST_PARTY`, with an online signing key one rotation link below it | §6 |
| D6 | A protocol conformance suite, run against the reference server, the service and any registry | §7 |
| D7 | Prerequisites are built by their owning tracks; this track adds only L1 (raw request bodies) and L2 (wasm behind an adapter) | §4 |

## 10. Candidate slice decomposition

Provisional until this doc settles. Each slice becomes an increment proposal,
filed as a sub-issue of this track's spine.

| Slice | Content | Needs |
|---|---|---|
| R0 | The conformance suite (D6), passing against `bynk registry serve` | — |
| L1 | Raw request bodies: `Bytes` handler parameters with content type and a cap | — |
| L2 | A wasm module as an adapter binding; the admission module built from the Rust code | — |
| (stdlib) | K4 `bynk.crypto` and `Random.bytes`; P5 R2 and byte streams; P3 rate limiting | #1936 |
| R1 | Service skeleton and read path: `config.json`, index files and archives from R2 with ETags; seeded by an operator import; conformance read checks pass | P5 |
| R2 | The publish API with operator-issued tokens: admission, the `PackageIndex` writer, signing, yank; full conformance passes | R1, L1, L2, K4 |
| R3 | Accounts and organisations: OIDC sign-in, claiming, members, claim-blocking, self-service tokens with expiry and revocation | R2 |
| R4 | Trusted publishing from CI | R3 |
| R5 | Abuse and operations: rate limits, quotas, the takedown procedure (Q4), the scheduled export, the deploy workflow with its conformance gate | R2, P3 |
| R6 | Go-live: the offline root and online key; `FIRST_PARTY` set in a toolchain release; the publication rule enforced; docs (a "Publish a package" guide); packaging §9 4b marked done | R3, R5 |
| R7 | Extraction to its own repository | R6, §1.3 |

## 11. Settling-review questions

1. **Q1, admission (D3).** Should admission be wasm from the Rust code, or
   reimplemented in Bynk? Reimplementing would add octet access and a TOML
   reader to `bynk-std`, which is more dogfood, but it creates two
   implementations of the most security-sensitive rules.
2. **Q2, the index writer (D2).** One agent per package, or one per
   organisation? Per package parallelises publishes. Per organisation makes
   quotas atomic.
3. **Q3, identity providers.** Is GitHub alone enough at launch? Is an email
   fallback needed for organisations without GitHub?
4. **Q4, takedowns.** Packaging promises that yanking never deletes. A legal
   takedown, or confirmed malware, must remove an archive. Proposed: an
   operator-only **tombstone**. The line stays, marked yanked. The archive is
   deleted, and fetching it fails as `bynk.registry.unavailable` with a reason
   instead of a checksum error. This needs a small protocol addition (a
   `withdrawn` reason on the line, not signed, like `yanked`). Is that
   acceptable, and does it belong to this track or to packaging?
5. **Q5, key custody (D5).** Is an offline root with an online signing key
   worth the ceremony at launch, or should launch pin the online key and add
   the root before 1.0?
6. **Q6, organisation names at launch.** Should well-known names be reserved
   up front (an impersonation list), or only on request?
7. **Q7, extraction trigger (§1.3).** Is "go-live plus two quiet `bynk-std`
   releases" right, or should extraction wait for 1.0?

## 12. Threat model

- **A leaked online signing key** lets the holder sign lines as `bynk` until
  rotated (D5 limits that to a rotation). Locked versions are safe anyway,
  because of the checksum pin.
- **A leaked publish token** can publish new versions only, under its own
  organisations, until it expires or is revoked. It can't change history,
  sign as the registry, or yank without the `yank` capability.
- **A malicious upload** goes through the shared admission rules (D3). The
  service only hashes, stores and signs it, and never executes uploaded code.
- **Dependency confusion** is closed structurally (packaging §6.2), and here by
  claim-blocking and the publication rule.
- **A compromised storage layer** can withhold or corrupt archives and index
  files. But clients verify every line's signature and every archive's
  checksum, so it can't substitute content (packaging §12).
- **Account takeover** is bounded by OIDC, by owner-only organisation
  administration, and by trusted publishing removing long-lived CI secrets.
- **The service's own supply chain** (E5): no registry dependencies. Its npm
  packages go through `bynk approve` like anyone's.

## 13. Sequencing

R0, L1 and L2 can start at once. The standard-library slices K4, P5 and P3
are already planned there, and this track asks only that they be ordered
early. R1 waits on P5. R2 waits on R1, L1, L2 and K4. R6 is a toolchain
release, so it lands with a version bump. Before 1.0, each track retirement is
a release trigger anyway.

## 14. Non-goals

- **A package search UI or web front end at launch.** The index is the API.
  A browsing site is later work, which can be a static page over the same
  files.
- **Download counts and analytics.**
- **Semver-honesty checks at publish** (packaging Q8, still deferred).
- **Mirrors run by Bynk.** Anyone may mirror; the protocol already verifies
  mirrors against the origin.
- **Changing the protocol** beyond Q4's `withdrawn` reason, if Q4 settles
  that way.
