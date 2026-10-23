# The first-party registry — a hosted Bynk service, written in Bynk

- **Status:** Settled (#2309, merged 2026-10-22): every §11 question (Q1–Q7)
  was closed with the maintainer, each on the proposed option, and Q5 was
  re-settled after review (revocable delegated keys, R5b). Slicing is next:
  R0, L1, L2 and R5b can start at once (§13). Spine
  [#2308](https://github.com/bynk-lang/bynk/issues/2308). This track is packaging
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
- **The surface wasn't settled.** Accounts, organisation claiming, trusted
  publishing, takedown policy and key custody each needed a decision, and two
  of them (D5, D8) add to the protocol (§11).
- **A security boundary.** It holds the root key whose public half ships in
  every `bynk` binary. A leaked root means a toolchain release (packaging
  §6.2); a leaked online signing key is revoked and replaced without one (D5).

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
  `bynk.toml`. It reaches only `bynk-std`, its own files, and git dependencies
  pinned by its lock (E5), never a relative path into the compiler's crates.
  Extraction is then a `git filter-repo` plus the toolchain pin it already
  deploys with (§8).
- **E2. No Rust inside the service.** Rust logic it shares (§3.4) crosses an
  adapter boundary, as if it were an external package. On extraction it
  becomes one.
- **E3. No account-specific configuration in git.** Cloudflare account IDs,
  secrets and the signing key are supplied at deploy through `Secrets` and CI
  secrets.
- **E4. Its own CI job.** `bynk check`, `bynk test`, and the conformance
  suite (§7) against a local `bynk dev`, on every PR. It deploys only from a
  separate, manually triggered workflow.
- **E5. It never depends on itself.** No registry dependencies. A dependency
  is a git dependency pinned by the lock, or a path inside
  `services/registry/` (a member of its own workspace). A broken registry, or a bad toolchain release, can then never stop
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
  src/registry/
    store.bynk              # context registry.store: owns the bucket; the read service (R1),
                            #   then the publish service and PackageIndex agents (R2)
    paths.bynk              # commons: object keys and the name rules guarding them
    admission.bynk          # adapter over the shared admission module (§3.4, R2)
  src/accounts/ …           # context: users, organisations, claims, tokens (R3)
  seed/seed.sh              # upload a `bynk registry` directory into the bucket
  tests/ …
```

**One context owns the bucket** (amended by R1, #2334). A bucket belongs to the
context that declares it (standard-library D3), so the read service, the
publish service, the `PackageIndex` writers and the reconciler all live in
`registry.store`; other contexts (accounts, R3) reach its data only through its
services. The sketch above first drew `read` and `publish` as separate
contexts, which D3 doesn't allow. **One bucket mirrors a static registry
directory exactly** — `config.json`, `<org>/<package>`, `dl/sha256/<hex>` — so a
seed is a directory copy and the protocol's paths map one-to-one onto keys.

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
   - **checks for an existing version.** An existing version with a different
     tree is `409`. One with the **same** tree is a retry, and gets `200` with
     the existing line (D10).
   - **stores the archive** at `dl/sha256/<hex>`. It is content-addressed, so
     a repeat is harmless.
   - **signs and appends the line in its own state,** which commits
     atomically with the handler (ADR 0109).
   - **writes the index file to R2.**
4. The response is `201` with the line, exactly as ADR 0570 specifies.

**Atomicity (D10).** The R2 writes are external effects, outside the agent's
atomic commit, so the order is chosen for what a failure leaves behind:
- **The archive write fails.** Nothing has been committed. The request fails
  (`503`) and can be retried.
- **The line commits, but the index-file write fails.** The line is not rolled
  back: the agent's state is authoritative, and the index file is a
  projection of it.
  - The request answers `503`.
  - The client's retry hits the "same tree" case (`200`), and the agent
    rewrites the index file on every call, so the projection catches up.
  - A scheduled reconcile also rewrites any index file whose projection lags.

So a line never exists without its archive, a publish is safe to retry, and a
reader never sees a line the writer didn't commit. Answering a same-tree retry
with `200` amends ADR 0570, which said `409` for any existing version. It
landed in R0 (#2311), in the spec, the reference server and the conformance
suite.

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

**Built (L2, #2321):** the admission path is the I/O-free
`bynk-registry-core` crate (`unpack`, `pack`, `admit` over an in-memory tree),
which `bynk registry add`/`serve` link natively. It also builds as a `cdylib`
with a plain C ABI and no `wasm-bindgen`: `alloc(len)`, `dealloc(ptr, len)`
and `admit(ptr, len, expected_ptr, expected_len) -> u64`, a packed pointer
(high 32 bits) and length (low 32) to a UTF-8 JSON result,
`{"ok": {name, version, checksum, deps, npm, archive}}` (the canonical archive
as base64) or `{"error": {code, message}}`. The module has no imports and is
one file, with no glue JS. `scripts/build-registry-core.sh` produces it; CI
holds it to a 1.5 MiB budget after `wasm-opt` and checks native/wasm parity on
the admission corpus. An adapter loads it through `includes`
on a binding (#2326).

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
| Byte streams | archives up to 64 MiB without buffering twice | Standard library (its L0 kernel layer) | stdlib P5 prerequisite |
| Rate limiting | abuse handling | `bynk.cloudflare` | stdlib P3 |
| A wasm module behind an adapter binding | shared admission (§3.4) | Emit / adapters | new, proposed here (L2) |

L1 and L2 are this track's own language slices. The rest are already on the
standard-library track's slice list (#1936). This track only asks for them to
be ordered earlier there (§13), and adds nothing to that track's scope.

## 5. Accounts, organisations and tokens

**Proposed (D4):**

- **People sign in with GitHub**, through GitHub's OAuth **device flow** (R3,
  #2363). The service keeps no passwords and no GitHub credentials. (This
  first said "an OIDC provider, through the `Oidc` actor". GitHub issues OIDC
  tokens only to Actions workflows, which is R4's case. People get OAuth 2.0
  with no ID token, so the mechanism was re-settled on #2363.)
- **An organisation** is claimed by a signed-in user, who becomes its owner.
  Owners add members, who may publish. Organisation names follow the manifest
  rule (`[a-z][a-z0-9-]*`).
- **Reserved names (Q6).** `bynk` is reserved for the toolchain (packaging
  §12). A short list is reserved at launch: toolchain and platform names
  (`bynk-*`, `cloudflare`, `workers`, `npm`, `node`, `deno`), and obvious
  impersonations of them. Anything else is first come, first served, with
  claim-blocking on request and a dispute route through the operators.
- **Claim-blocking** (packaging §6.2). A private organisation may reserve its
  name without publishing, so an unrouted `acme` can never resolve to a
  stranger's `acme`. A blocked name serves no index, and publishes to it are
  refused.
- **Tokens** are opaque, random (`Random.bytes`), stored as SHA-256 digests,
  and scoped to organisations and to `publish` or `yank`, exactly as in the
  reference server (ADR 0571 E). Unlike there, they also expire and can be
  revoked.
  People mint them through the account API (R3, #2363). The operator-issued
  table of R2 (#2336), the `REGISTRY_TOKENS` secret, stays as the bootstrap and
  CI path, and its tokens don't expire.
- **Trusted publishing.** A CI job publishes with its OIDC token (for example
  GitHub Actions'), exchanged for a short-lived publish token. The token is
  bound to the repository and workflow that the organisation configured. No
  long-lived secret sits in CI.
- **The publication rule** (packaging §6.2). Admission refuses a package
  whose dependencies would resolve outside the first-party registry, meaning
  any dependency whose organisation isn't a first-party organisation.

Publishing needs no client change for any of this: `bynk publish` sends a
bearer token, and how the token was issued is the service's business. Managing
an account does: `bynk login`, `bynk logout` and `bynk org` drive the account
API, keep the account token in the credentials file's `[accounts.<name>]`, and
can save a minted publish token where `bynk publish` reads it (R3.1, #2378).

## 6. Operating it

- **Key custody (D5): an offline root, delegated signing keys, and
  revocation.**
  - **Why the protocol needs a change.** Today's chain can't recover from a
    leaked key. `keys` is a linear chain, each link signed by the previous
    key, and a client accepts every key the chain reaches from its anchor
    (ADR 0564). A leaked key can't be removed, and its holder can append links
    of their own (`registry-protocol.md`, "Rotation covers planned rotation
    only"). An offline root above one online key would therefore buy nothing.
    So this track adds **delegation and revocation** to the protocol (slice
    R5b):
  - **The design:**
    - **`delegations`** in `config.json`. Each entry is
      `{ "key": "ed25519:…", "sig": … }`, where `sig` is a chain key's
      signature over `{"delegate":"<key>","registry":"<id>"}`. A delegated key
      may sign index lines. It may not extend the chain, delegate, or revoke.
    - **`revoked`** in `config.json`. Each entry is `{ "key": …, "sig": … }`,
      where `sig` is a chain key's signature over
      `{"revoke":"<key>","registry":"<id>"}`. A revoked delegate's signatures
      are refused from then on, including on lines it signed before (they
      fail as `bynk.registry.bad_signature`).
    - **Re-signing.** `sig` joins `yanked` as a field of a line that may
      change, but only to a valid signature over the same payload. Recovery is
      then: revoke the leaked delegate, delegate a new key, and re-sign every
      line with it.
    - **Existing registries are unaffected.** A registry without `delegations`
      behaves exactly as today, with lines signed by chain keys.
  - **For the first-party registry:**
    - the chain is the **root**, kept offline, and only its public key ships
      in `FIRST_PARTY`;
    - the online signing key is a delegate, held only in the service's
      `Secrets` and imported at deploy;
    - a leaked delegate costs an offline ceremony (sign a revocation and a new
      delegation) and a re-sign of every line, but no toolchain release;
    - only a leaked root needs a release.
  - **What a lock still guarantees.** Locked versions are unaffected
    throughout, because of the checksum pin.
  - **What it doesn't cover: freshness.** A party that can serve a stale
    `config.json` (without the revocation) can keep a revoked delegate alive
    for clients who read through it. The first-party registry is read over
    TLS from its origin, and timestamped freshness is the TUF depth packaging
    §11 Q10 deferred.
- **Deploys** come only from a manual workflow with an approval gate (E4).
  Each deploy runs the conformance suite against staging first.
- **Durability.** Archives and index files are in R2. Each index line is also
  kept in the agent's own state, so an index file can always be rebuilt from
  its writer. There's a scheduled export of both.
- **Abuse.**
  - Upload size caps (64 MiB, as the reference server).
  - Per-token and per-IP rate limits (P3).
  - A per-organisation storage quota.
- **Takedowns (Q4, D8).** Yanking never deletes (packaging §12), but a legal
  takedown or confirmed malware must. An operator-only **tombstone** handles
  it:
  - the line stays, marked `yanked`, with an unsigned
    `"withdrawn": "<reason>"` (unsigned, like `yanked`, so it re-signs
    nothing);
  - the archive is deleted;
  - a client fetching it fails as `bynk.registry.withdrawn`, naming the
    reason, instead of with a checksum or HTTP error. (R5a.1, #2330, gave the
    failure its own code rather than `bynk.registry.unavailable`, so scripts
    and CI can tell a takedown from an outage.)

  **Where it applies.** `withdrawn` is honoured when a client reads the index,
  so it applies to resolving and fetching:
  - a fresh clone, `bynk update`, or a lock whose tree isn't in the store fails
    with the reason;
  - a locked build whose tree is already in `$BYNK_HOME` (checksum-verified)
    reads no index, and keeps working until its store entry is gone.

  The service can't reach into users' machines, and the reason tells them what
  to remove. Like `yanked`, `withdrawn` is unsigned, so a mirror or a
  compromised storage layer could add one. That gives no new power to
  substitute content, since either could already withhold the archive. But it
  could dress a withhold up with an authoritative-looking reason. So a client
  reports `withdrawn` **attributed to where it read it**: read through a
  mirror, the message names the mirror, and says the origin hasn't been
  consulted. This is a small protocol addition, owned by this track as its own
  slice (R5a). The client learns to read `withdrawn`, and the conformance
  suite checks it.

## 7. Conformance

**Proposed (D6):** a conformance suite in this repository (Rust, beside the
registry tests) that takes a registry URL and a token. It runs the protocol's
observable behaviour against it:
- `config.json`'s shape and key chain;
- a publish, read back and verified;
- `409` on a republish of a different tree, and `200` with the existing line
  on a retry of the same tree (D10);
- delegation, revocation and re-signing (D5), once R5b lands;
- `401` and `403` on bad tokens;
- `400` on a mismatched or traversing upload;
- `413` past the cap;
- yank and unyank;
- a consumer resolving and locking what was published.

The reference server passes it in CI. The service passes it against
`bynk dev` on every PR, and against staging before every deploy. A private
registry operator can run it too.

## 8. Self-hosting constraints

- **E5 again.** No registry dependencies, ever, so the service can always be
  built.
- **Toolchain (D11).** In this repo, CI builds it with the in-tree toolchain
  (that's the dogfood), so a capability lands in the same PR as its first use.
  Production deploys **from a release tag**, never from `main`, building with
  the toolchain released at that tag. Releases tag `main` (#1673), so the
  tag's service and toolchain always agree. Service code that uses an
  unreleased capability is simply not deployable until the next release. The
  deploy workflow (R5) refuses any ref that isn't a release tag. On extraction
  (R7), the service pins the released toolchain it deploys with, and bumps it
  deliberately.
- **The registry isn't on the fetch path of its own build,** and `bynk`'s
  read path doesn't depend on any of the service's write-side state. A
  write-side outage leaves resolution working.

## 9. Decisions

| # | Decision | § |
|---|---|---|
| D1 | The service is written in Bynk, in this repo at `services/registry/`, under extraction rules E1–E5, and extracted after go-live | §1.3 |
| D2 | A `PackageIndex` agent per package is the single writer; an archive is stored before its line | §3.3 |
| D3 | Admission is the Rust `admit` path compiled to wasm behind an adapter, not a reimplementation | §3.4 |
| D4 | GitHub sign-in (the OAuth device flow; re-settled on #2363 from "OIDC"), owner-and-member organisations, claim-blocking, hashed scoped expiring tokens, trusted publishing from CI (OIDC) | §5 |
| D5 | An offline root pinned in `FIRST_PARTY`; online signing keys are root-signed **delegates**, revocable by a root-signed **revocation**, with lines re-signed on recovery: a protocol addition (R5b) | §6 |
| D6 | A protocol conformance suite, run against the reference server, the service and any registry | §7 |
| D7 | Prerequisites are built by their owning tracks; this track adds only L1 (raw request bodies) and L2 (wasm behind an adapter) | §4 |
| D8 | Takedowns are an operator-only tombstone: the line stays, yanked, with an unsigned `withdrawn` reason, and the archive is deleted; a protocol addition owned here | §6 |
| D9 | GitHub is the only sign-in at launch (OAuth device flow for people, OIDC for CI); a short reserved-name list plus claim-blocking on request | §5 |
| D10 | A same-tree retry of a publish is `200` with the existing line, not `409`; the agent's committed state is authoritative and the R2 index file is a projection the agent and a scheduled reconcile keep current | §3.3 |
| D11 | CI builds the service with the in-tree toolchain; production deploys only from release tags, with that tag's released toolchain | §8 |

## 10. Candidate slice decomposition

Provisional until each slice's proposal is accepted. Each slice becomes an increment proposal,
filed as a sub-issue of this track's spine.

| Slice | Content | Needs |
|---|---|---|
| R0 | The conformance suite (D6), passing against `bynk registry serve`. **Done** (#2311): `bynk registry conformance`; D10's idempotent retry landed with it | — |
| L1 | Raw request bodies: `Bytes` handler parameters with content type and a cap. **Done** (#2320): `@raw(contentType:)`, a `415` for another type, a required cap counted on the bytes read (JSON bodies too), a `Signature` actor over the octets | — |
| L2 | A wasm module as an adapter binding; the admission module built from the Rust code. **Done** (#2321): `binding … includes ["./x.wasm"]`, and `bynk-registry-core` | — |
| L3 | Caching raw responses: validators for `Raw`, and `immutable`. **Done** (#2335): a `Raw` `GET` body of at most 1 MiB gets the weak `ETag`, `RawTagged(body, contentType, etag)` sends a handler-supplied one, `@cache(…, immutable: true)` | — |
| L4 | An external HTTP contract (#2341): a handler reads the request's headers, answers an error status with its own body, and renders the router's rejections. **Done:** `headers: RequestHeaders` (`get`, `bearer`, `clientIp`), `RawError(status, body, contentType)` with `ErrorStatus` names, and `on rejected` for the router's `400`/`405`/`413`/`415` (actor-seam `401`s are a follow-on) | — |
| (stdlib) | K4 `bynk.crypto` and `Random.bytes`; P5 R2 and byte streams; P3 rate limiting | #1936 |
| R1 | Service skeleton and read path: `config.json`, index files and archives from R2; seeded by an operator import; conformance read checks pass. **Done** (#2334): one owning context, `seed/seed.sh`, the `registry-service` CI job | P5 |
| L3 | Caching for `Raw` responses: a weak `ETag` for small `Raw` bodies, a handler-supplied one (`RawTagged`), `@cache(immutable:)` — the read paths serve `Raw`, which carries no validator and can't be `@cache`d today | — |
| R2 | The publish API with operator-issued tokens: admission, the `PackageIndex` writer, signing, yank; full conformance passes. **Done** (#2336): admission and signing bytes from `bynk-registry-core` through a wasm adapter, a checked online delegate, `scripts/check-publish.sh` (full conformance, the real client) in CI | R1, L1, L2, L4, K4 |
| R3 | Accounts and organisations: GitHub sign-in, claiming, members, claim-blocking, self-service tokens with expiry and revocation. **Done** (#2363): GitHub device-flow sign-in, `Organisation`/`PublishToken`/`Session` agents, reserved names, two token kinds, operator tokens kept, `scripts/check-accounts.sh` in CI | R2 |
| R3.1 | The account client: `bynk login`, `bynk logout`, `bynk org` (claim, show, block, members, tokens). **Done** (#2378): device-flow polling per RFC 8628, the account token in `[accounts.<name>]` and `BYNK_ACCOUNT_<NAME>_TOKEN`, `token mint --save` into the publish slot, no account token through a mirror | R3 |
| R4 | Trusted publishing from CI | R3 |
| R5 | Abuse and operations: rate limits, quotas, the scheduled export, the release-tag-only deploy workflow (D11) with its conformance gate | R2, P3 |
| R5a.1 | Takedowns (D8), the protocol half: the unsigned `withdrawn` field in the protocol spec, the client's reading of it (`bynk.registry.withdrawn`), `bynk registry withdraw`, the `410`/`409 withdrawn` refusals, and the conformance check. **Done** (#2330) | — |
| R5a.2 | Takedowns (D8), the hosted half: the operator tombstone inside the service, its operator tooling and audit trail | R2, R5a.1 |
| R5b | Key delegation and revocation (D5): `delegations` and `revoked` in `config.json`, re-signable `sig`, the client's verification rules, operator tooling for the offline ceremony, `bynk registry serve` support, conformance checks. **Done** (#2316): protocol 2; `rotate`, `delegate`, `revoke`, `resign` | — |
| R6 | Go-live: the offline root and online key; `FIRST_PARTY` set in a toolchain release; the publication rule enforced; docs (a "Publish a package" guide); packaging §9 4b marked done | R3, R5, R5a, R5b, L3 |
| R7 | Extraction to its own repository | R6, §1.3 |

## 11. Settling-review questions (all resolved)

Each was closed with the maintainer on 2026-10-22, on the proposed option.

1. **Q1, admission. Resolved: wasm from the Rust code (D3).** The client, the
   reference server and the service apply one implementation of the
   admission rules. Reimplementing in Bynk was rejected: it would create two
   implementations of the most security-sensitive code, and need a parity
   suite to keep them in step. Octet `Bytes` access and a TOML reader stay on
   the standard-library track's own merits.
2. **Q2, the index writer. Resolved: one `PackageIndex` agent per package
   (D2).** Publishes to different packages run in parallel. The
   per-organisation quota lives in a per-organisation `OrgUsage` agent. Each
   `PackageIndex` consults it before storing and adds to it after committing.
   Those are two calls, not one atomic commit, so the quota is best-effort at
   the margin: concurrent publishes to different packages can each pass the
   check, and an organisation can exceed its quota by at most one upload per
   package in flight.
3. **Q3, identity providers. Resolved: GitHub OIDC only at launch (D9).** It
   matches trusted publishing from GitHub Actions (R4). An email fallback was
   rejected for launch: it needs an email capability and widens the abuse
   surface. Other providers come later behind the same `Oidc` actor.
4. **Q4, takedowns. Resolved: an operator tombstone, owned here (D8, §6,
   slice R5a).** The line stays, yanked, with an unsigned `withdrawn` reason,
   and the archive is deleted. Clients say why. Deleting without a protocol
   change was rejected because of the unexplained failures it causes. Routing
   the amendment through the packaging track was rejected as ceremony, since
   that track is complete apart from 4b.
5. **Q5, key custody. Resolved, then re-settled after review: an offline
   root with revocable delegated signing keys, which is a protocol addition
   (D5, R5b).** The first resolution, an offline root with the online key one
   chain link below it, didn't hold. The review of #2309 showed that the
   protocol's linear chain can't revoke a key, so the root bought nothing.
   Scaling D5 down (pinning the online key, with a leak needing a toolchain
   release) and short-lived expiring delegations were the alternatives. Both
   were rejected in favour of explicit revocation, which keeps the root's
   value and leaves registries without delegations unchanged.
6. **Q6, reserved names. Resolved: a short list plus claim-blocking on
   request (D9, §5).** A broad up-front list was rejected: it needs a dispute
   process before there are users to dispute.
7. **Q7, the extraction trigger. Resolved: go-live plus two consecutive
   `bynk-std` releases that needed no service change (§1.3).** Extracting at
   1.0 keeps lockstep longer than needed. Extracting right after go-live means
   cross-repo changes while `bynk-std` still moves.

## Front-loaded ADRs

Numbers are taken at merge (ADR 0076).

1. **The first-party registry's architecture** (D1–D3, D6): written in Bynk,
   in this repo under E1–E5, a single writer per package, shared wasm
   admission, and the conformance suite. Lands with R0 or R1, whichever is
   first.
2. **Key delegation and revocation** (D5): the protocol addition and the
   first-party custody model. Lands with R5b.
3. **Accounts, organisations and tokens** (D4, D9). Lands with R3.
4. **Takedowns** (D8): the `withdrawn` protocol addition. Lands with R5a.

## 12. Threat model

- **A leaked online signing key** (a delegate) lets the holder sign lines as
  `bynk` until the root revokes it (D5). After that, its signatures are refused,
  and every line is re-signed by a new delegate. A party serving a stale
  `config.json` can delay the revocation for its readers (freshness, deferred
  TUF depth). Locked versions are safe anyway,
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
  checksum, so it can't substitute content (packaging §12). It, or a mirror,
  can also add the unsigned `yanked` or `withdrawn` fields. That is a withhold
  with a message, not a substitution, and a client attributes a `withdrawn`
  to the mirror it came through (D8).
- **Account takeover** is bounded by OIDC, by owner-only organisation
  administration, and by trusted publishing removing long-lived CI secrets.
- **The service's own supply chain** (E5): no registry dependencies. Its npm
  packages go through `bynk approve` like anyone's.

## 13. Sequencing

R0, L1, L2 and R5b can start at once (R5b is client and protocol work in Rust, and needs nothing from `bynk-std`). The standard-library slices K4, P5 and P3
are already planned there, and this track asks only that they be ordered
early. R1 waits on P5. R2 waits on R1, L1, L2 and K4. L3 (caching `Raw`
responses, added by R1) can run at any time, and R6 waits on it: the read
paths are correct uncached, but a public registry needs its archives cached at
the edge. R6 is a toolchain
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
- **Changing the protocol** beyond D5's delegation and revocation, D8's
  `withdrawn` reason, and D10's same-tree retry.
- **Sign-in providers beyond GitHub at launch** (D9). Others come later
  behind the same `Oidc` actor.
