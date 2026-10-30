# The Bynk standard library — layers, placement, and admitting the Workers surface

- **Status:** Slicing. Direction settled by #1938 (all of §8 D1–D9 and §11
  Q1–Q7 closed). S0 done (#1940): the first-party sources live in `bynk-std`.
  S1 done (#1945): a `bynk-std` registry drives their injection (R-6 met).
  S2 done (#1952): every binding is type-checked and behaviour-tested per
  platform and per operation (R-12, R-13 met for the shipped bindings).
  S3 done (#1963): stable/experimental marking with the `[unstable] allow`
  opt-in, extensible enums (`FetchError`, `Method`), the D2 tiers ADR and the
  Gate 1 amendment (R-17 met; §11 Q5, Q6 built). S4 done (#1966, #1967):
  derived KV capabilities, built and deployed. K1 done (#2016): `Bytes` joins,
  slices, compares, and converts to and from hex and base64url. P2 done
  (#2578): a context sends to a Cloudflare Queue through a named `Queue`.
  P3b done (#2582): a context writes events to a Workers Analytics Engine
  dataset through a named `Analytics`.
  K5a done (#2586): more `Method`s, `Fetch.sendWith` with a timeout and a
  redirect policy, and `Logger` levels with a structured `log`.
  P1 done (#2590): `Kv` gains metadata, expiry, binary values and paging.
  K3 done (#2592): `Instant` reads and writes RFC 3339 text.
  K6b done (#2598): integer `div`/`rem`/`mod`/`pow` and `Float` functions.
- **Realises:** `design/bynk-design-notes.md` "Three-tier output" (the runtime
  library and per-platform bindings, including the unbuilt `Crypto`),
  [ADR 0034](../decisions/0034-collections-hybrid-kernel-stdlib.md)'s promise
  that "the stdlib can grow without compiler changes", and
  `design/bynk-greenfield-compiler.md` Part 14 (the capability tier taxonomy).
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md).
  Qualifies on all three axes (§1.2).
- **What's already true:** a stdlib in miniature, at v0.330.x, surveyed
  2026-10-14. It has a kernel of built-in types and methods, four Bynk-written
  commons, the `adapter bynk` surface, and a `bynk.cloudflare` adapter offering
  a single fixed KV namespace. They are all embedded via `include_str!`, from
  `bynk-check/src/firstparty/` when this track opened and from the data-only
  `bynk-std` crate since S0, and injected by seven hand-coded blocks in
  `project_model.rs` until S1 replaced them with a registry. Spine [#1936](https://github.com/accuser/bynk/issues/1936).
- **Governing records:** ADR 0012 (the `bynk` surface is ambient-only), 0016
  (no portable infrastructure), 0023 (platform work and stdlib work never share
  an increment), 0024 (platform nativity is metadata), 0026 (platform adapters
  at `bynk.<platform>`), 0034/0036/0037 (thin kernel, Bynk-written combinators),
  0045 (typed JSON codec, no untyped `Json`), 0086 (first-party sources are
  embedded files, vendored), 0310 (only the four-item emit ABI is published).

## 1. The theme

### 1.1 What this track settles

As the toolchain stabilises, Bynk needs a deliberate standard library for a
runtime that is a Cloudflare Worker. This track defines what the standard
library *is* (§2), where it sits (§3), what may join it (§4–§5), what is
missing (§6), what must happen before any of it is built (§7), and the
decisions that shape it (§8). The work is then cut into slices (§9).

In short:

1. **"The standard library" is four layers, not one thing.** Two are portable
   and two are Cloudflare-shaped, and existing ADRs keep those apart on
   purpose. "Everything a Worker has" maps almost entirely onto the platform
   layer (`bynk.cloudflare`). The portable core has its own, smaller gap list.
2. **It ships in the `bynk` distribution**, from a new data-only `bynk-std`
   crate (D1), with a tiered 1.0 stability promise (D2). The platform layer
   stays bundled, but could be split out later without renames (D9).
3. **Several Worker features are not stdlib at all.** Inbound handlers,
   Durable Objects, service bindings and `ctx.waitUntil` are language or
   runtime work (§6.4).
4. **Platform resources get context-owned, named instances** through derived
   capabilities, `capability Sessions = Kv` (D3). Structured payloads use a
   two-step `Json.decode` (D4). Crypto is effectful, over Web Crypto (D5), in
   its own `bynk.crypto` unit (D6).

**End state when this track retires:**
- a data-driven first-party registry in `bynk-std`;
- stable/experimental marking backing the 1.0 promise;
- derived capabilities, with deploy provisioning for every instantiable kind
  admitted so far;
- a documented, CI-checked admission process (§4–§5) that later additions
  follow without a new track;
- the first wave of core and platform slices (§9) landed.

The Workers surface is not expected to be *complete* at retirement (§14).

### 1.2 Why a track (the ADR 0076 trigger)

- **Multi-increment.** Four prerequisite refactors, a language increment
  (derived capabilities), and a long run of core and platform slices. ADR
  0023 forbids mixing stdlib and platform work in one increment.
- **Surface not yet settled.** The resource-instance model, the stability
  promise and the crypto purity rule are new surface or policy.
- **Security/safety boundary.** `bynk.crypto` handles keys, signatures and
  HMAC. Deploy provisions and prunes named resources holding production data
  (§12).

## 2. What the standard library is

| Layer | What | Where today | Portable? | Grows by |
|---|---|---|---|---|
| **L0 Kernel** | Irreducible primitives: base types, built-in type constructors, kernel methods, compiler-backed statics (`Json.decode[T]`) | Hard-coded in `bynk-syntax` (`ast.rs`), `bynk-check/src/kernel_methods.rs`, `resolver.rs`, emitter lowering | Yes | Compiler change + ADR |
| **L1 Commons** | Everything derivable from L0, written in Bynk | `bynk.list`, `bynk.map`, `bynk.string`, `bynk.locale(.types)` under `bynk-std/lib/core/` (S0) | Yes | `.bynk` source only (ADR 0034's promise) |
| **L2 Ambient surface** | Capabilities every platform guarantees: `Clock`, `Random`, `Logger`, `Fetch`, `Secrets`, (`Locale`, `Idempotency`, `Events`) | `adapter bynk` + one TS binding per platform (cloudflare/node/browser) | Yes, and it is the portability marker | `.bynk` + three bindings |
| **L3 Platform adapter** | Honest, platform-shaped infrastructure capabilities | `adapter bynk.cloudflare` (`Kv` only) + `cloudflare.binding.ts` | No; it locks the deployment unit to Cloudflare (ADR 0017) | `.bynk` + one binding + wrangler derivation + deploy provisioning |

The **runtime library** (`bynk-emit/runtime/src` → `runtime.ts`) sits
underneath all four. It is codegen support, not user-visible surface, and this
spec does not treat it as part of the standard library. It is affected only
where a new layer needs runtime help, such as the crypto verifiers in `auth.ts`.

**Layering requirement.** Every proposed stdlib item must name its layer, and §4 says
how a layer is chosen. An item that fits no layer is out of scope for this
track.

## 3. Where the standard library sits

### 3.1 Position

The stdlib **ships with the `bynk` distribution**, which matches your instinct
and shipped practice:

- The sources are real files embedded by `include_str!` into the toolchain
  binary (ADR 0086). They are injected into a project only when it `uses` or
  `consumes` them, and their bindings are vendored into the build output. The
  compiler that emits your code therefore ships the stdlib it was checked
  against, and stdlib/compiler skew cannot happen.
- L0 cannot be anywhere else: it *is* the compiler.
- No alternative exists today: there is no manifest dependency, no registry and
  no packaging identity model. The packaging track has a spine
  ([#843](https://github.com/accuser/bynk/issues/843)) but no track doc yet.

### 3.2 Requirements on placement

- **P-1 Bundled, every binary.** Every stdlib layer is available to all four
  toolchain artefacts that embed the pipeline: `bynkc`, `bynk`, `bynkc-lsp` and
  `bynk-wasm` (the playground). Today `bynkc`, `bynk` and `bynkc-lsp` ship as
  GitHub Release binaries, and the library crates go to crates.io.
  `bynk-wasm` is built for the playground and is in neither channel.
  Nothing Bynk-owned is on npm except `tree-sitter-bynk`.
  "Part of the `bynk` distribution" means these channels, not an npm package.
  The stdlib needs no network fetch and no npm install of Bynk-owned
  packages. Builds stay hermetic, except for any npm `requires` a
  binding declares (ADR 0013).
- **P-2 Package-shaped sources.** The stdlib sources live in a
  self-contained tree that can be extracted later as a unit without rewriting
  it: `.bynk` sources, TS bindings, per-unit metadata and tests. The tree is
  owned by the `bynk-std` crate, whose only job is to hold the stdlib,
  not by `bynk-check` (D1, settled). The checker consumes a registry; it does not contain
  the library.
- **P-3 Version identity.** The stdlib has no separate version from the
  toolchain while it is bundled. Any later unbundling, most plausibly of L3,
  must be possible without renaming units (`bynk.cloudflare` stays
  `bynk.cloudflare`) and without changing user source.
- **P-4 Only the emit ABI is published.** Publishing anything beyond ADR 0310's
  four-item ABI (`Result`/`Option` tag layout, `JsonError`, `Uuid.of`,
  `FetchError`) is out of scope. A stdlib binding that references a codegen
  shape outside that list is a defect (ADR 0310 D4).
- **P-5 Pay for what you use.** A project that does not `uses`/`consumes` a
  unit emits byte-identical output with or without it. This is today's
  property, and it must survive the move to a registry.

### 3.3 The placement decisions

D1 (owning crate), D2 (stability promise) and D9 (unbundling L3 later) are
all settled in §8.

## 4. Admission rules

How a candidate is placed. These apply existing ADRs; they are not new policy.

1. **L0 or L1?** It is L0 only if it cannot be written in Bynk over the
   existing kernel: it needs iteration (Bynk has no loops), per-type synthesis
   (the `Json.decode[T]` pattern) or a JS primitive with no Bynk equivalent.
   Everything else is L1 (ADR 0034/0036). A new kernel method needs an ADR
   saying why it is irreducible.
2. **Pure computation or capability?** An operation is pure (L0/L1) only if
   Bynk can implement it **synchronously on every platform without
   third-party code**: URL parsing, hex/base64url, percent-encoding,
   ISO-8601. Anything that touches the environment (time, randomness, I/O,
   secrets, platform resources) is a capability. So is anything whose
   platform implementation is async, because `Effect[T]` lowers to
   `Promise<T>` and pure functions lower synchronously: hashing, HMAC and
   signing (D5).
3. **L2 or L3?** It is L2 only if its guarantee holds on every supported
   platform (Cloudflare, Node, browser), or can be weakened until it does
   (Part 14 R14.5). Infrastructure never joins L2 (ADR 0012), and portable
   infrastructure is never built (ADR 0016).
4. **Can it be a capability at all?** It must pass Part 14's test: no syntax,
   no compile-time-checked payload, no handler-transaction participation and
   no compiler-synthesised argument. Anything that fails a clause is
   language work, routed out of this track. SQL text checking and typed AI
   model I/O are the likely failures (§6, D4).
5. **Tier closure.** An L2 capability may depend only on L2 capabilities, and
   an L3 one on L2 or L3 (R14.3).

## 5. Cross-cutting requirements

Every stdlib addition, in any layer, must meet these.

### Surface

- **R-1 Effects are explicit.** Every op that touches the environment returns
  `Effect[…]`, and failures the caller can act on are `Result[_, E]` with a
  closed error enum (precedent: `FetchError`). No op throws for a recoverable
  condition. Contract violations may throw, but they are documented
  normatively (precedent: ADR 0045 on non-finite floats).
- **R-2 Honest semantics.** The doc comment states consistency, ordering,
  durability, limits and failure modes as the platform actually behaves. Two
  examples: KV is eventually consistent, and `Clock` may not advance between
  I/O operations on deployed Workers. Lowest-common-denominator wrappers are
  not allowed (ADR 0016).
- **R-3 Typed boundaries, no untyped JSON escape hatch** unless ADR 0045 is
  explicitly revisited (D4). Payloads cross as Bynk types through the existing
  per-type codec.
- **R-4 Naming and shape follow the kernel's established conventions:**
  `Option` for absence, method forms over free functions (ADR 0116), statics
  for constructors (`Bytes.fromUtf8`), `Duration` rather than raw seconds for
  time quantities. The shipped `Kv.putTtl(…, ttlSeconds: Int)` (ADR 0051,
  v0.23) predates `Duration` (ADR 0112). Reconcile it under D2's stability
  call. ADR 0051 already names the `putWith(options)` record as the planned
  successor once KV write options cluster.
- **R-5 No ambient globals.** User code reaches a platform resource only
  through a capability it `consumes`. `env` is read only inside first-party
  platform bindings (ADR 0018/0025).

### Delivery and build

- **R-6 Registry-driven.** Adding a unit is data plus files: unit name, kind,
  dependency units, per-platform bindings, platform nativity, env-taking
  providers and wrangler derivation. It needs no new branch in
  `project_model.rs` (prerequisite PR-1).
- **R-7 Wrangler derivation.** Every L3 capability declares the
  `wrangler.toml` table it requires, and the emitter emits a stanza only for
  an instance the context actually *references* (in a `given` or an op
  call). For KV this already held: `needs_kv` (`bynk-emit/src/project.rs`)
  walks only what handlers and providers are `given`, so a context that
  consumes `Kv` without using it gets no stanza. S4a keeps derived instances
  out of that walk (D3 rule 4).
- **R-8 Deploy provisioning.** `bynk deploy` provisions or links every
  resource an L3 capability needs (create a D1 database, R2 bucket, queue…), or
  the capability's doc states exactly what the user must do by hand. Resource
  naming must not orphan provisioned state across a later rename (cf. the
  packaging-track caveat in `bynk-adoption-sequencing.md`).
- **R-9 Local dev.** Every L3 capability works under `bynk dev`
  (wrangler/Miniflare local simulation), or its doc says it does not and the
  dev command fails clearly.
- **R-10 Platform lock.** L3 units are registered platform-native (ADR 0024),
  so consuming them from a `--platform node|browser` build is a diagnostic,
  not a runtime failure.
- **R-11 Parity for L2.** A new L2 capability ships bindings for all three
  platforms in the same increment, with any guarantee differences stated
  (R14.5). The runtime baseline is declared and CI-verified.

### Quality

- **R-12 Standing gates.** `.bynk` sources parse, check and are
  `bynk-fmt`-clean. TS bindings pass `tsc --strict` *standalone*, not only
  transitively. ADR 0086 lists this standalone scaffold as an open follow-up,
  and it becomes prerequisite PR-3. Every positive fixture passes the
  `tsc_verify` gate.
- **R-13 Testability.** Every capability is mockable in `bynkc test`
  (`mocks`/`expect … called`). Each op has a behavioural test of its binding
  against a contract-faithful fake (mutation-checked per op), plus a
  Miniflare-backed test where the platform semantics matter.
- **R-14 Documentation is part of done.** Doc comments on every exported
  item, a generated or hand-maintained reference page per unit in the book,
  and a changelog entry. The proposals README's docs-delta rule applies.
- **R-15 Tooling currency.** Hover, completion, signature help, semantic
  tokens and go-to-definition work into stdlib sources (ADR 0156's four
  named surfaces, plus navigation into embedded files).
- **R-16 Security review** for anything handling keys, signatures, secrets or
  untrusted bytes (crypto, mTLS, HTMLRewriter input). ADR 0087's posture
  applies.

### Stability

- **R-17 Stability is marked per item.** Each exported item is *stable* or
  *experimental*. Using an experimental item requires an explicit opt-in in
  `bynk.toml`, e.g. `[unstable] allow = ["bynk.cloudflare.D1"]` (§11 Q6).
  Using an unlisted experimental item is an error whose suggestion adds the
  line, so the edge of the 1.0 promise is visible in the manifest. A warning
  per use was rejected because warnings cannot be silenced today, so it would
  be constant noise. The marker is first-party metadata, not syntax (the ADR
  0024 pattern), until third parties need it. This is what lets the Workers
  surface land incrementally, including Cloudflare's own beta products,
  without every op becoming a 1.0 promise.
- **R-18 Deprecation path.** Removing or reshaping a stable item follows the
  ADR 0116 pattern (deprecate with a diagnostic and a suggestion, then
  remove), with a post-1.0 codemod where the release discipline requires one.
  After 1.0, removal waits for a major release, apart from D2's platform
  carve-out.

## 6. Inventory

Provenance: statuses come from a code survey of v0.330.4 on 2026-10-14. The
binding and runtime-API lists come from Cloudflare's documentation index on
the same date. Rows marked "verify" were not confirmed against either source.

Status key: **S** shipped · **P** partial · **A** absent · **D** designed but
unshipped (an ADR or design note names it). "Home" is the proposed layer or
unit and is indicative only, not decided.

### 6.1 Portable core gaps (L0–L2)

| Item | Status | Home | Notes / prerequisite |
|---|---|---|---|
| Hashing (SHA-256/384/512), HMAC, constant-time compare | A (internal only, in `runtime/auth.ts`) | L2 `bynk.crypto` `Crypto` (D5/D6), `Effect`-returning over Web Crypto | Design notes name `Crypto`. Security review (R-16) |
| Signing/verification (Ed25519, ECDSA, RSA), JWK import | A | L2 `bynk.crypto` `Crypto` | Key material is a secret: interacts with `Secrets` |
| Random bytes | A (`Random.uuid`/`int` only) | L2 `Random` | Trivial extension |
| Hex, base64url, percent-encoding | P (base64, hex, base64url and UTF-8 on `Bytes`, K1 #2016; percent-encoding is K2) | L0 statics | Pure |
| URL parse/build, query strings | A | L0 type + L1 helpers | Needed by Fetch users and HTTP handlers |
| `Set[T]` value type | A | L0 | Store kind `Set` exists; value type does not |
| Math (`pow`, `sqrt`, `log`, trig, integer `div`/`mod` semantics) | S (K6b #2598: `Int` `div`/`rem`/`mod`/`pow`, faulting on a zero divisor or past the safe range; `Float` `sqrt`/`pow`/`exp`/`ln`/`log10`/`log2`/trig/`atan2`, host-defined like `Float` arithmetic; `Float.pi()`/`Float.e()`) | L0 value methods (not `Math` statics: the numeric kernel is methods) | A `%`/`**` operator is grammar work |
| Regex match/capture/replace | A (`Matches` refinement only) | L0 | Engine/flag semantics must be stated (JS `RegExp`) |
| ISO-8601 parse/format for `Instant` (UTC) | S (K3 #2592: `Instant.fromIso`, `toIso`, RFC 3339 in UTC, offsets converted) | L0 | Pure. Time zones and calendar fields deferred (D7) |
| String formatting (pad, repeat, `codePoints`) | P | L0/L1 | |
| `Bytes` ops (concat, slice, compare, hex) | S (K1 #2016; octet access and orderability are follow-ons) | L0 | Prerequisite for R2, crypto, binary Fetch bodies |
| Fetch: more methods (`Patch`/`Head`/`Options`), `Bytes`/stream bodies, redirect & timeout options, `AbortSignal` | P (K5a #2586: `Patch`/`Head`/`Options`; experimental `sendWith(req, FetchOptions { timeout, redirect })`. Still `String` bodies: `Bytes` bodies are K5b, streams K-streams) | L2 `Fetch` | ADR 0022/0453 shape. Options go in a record beside `send`, never in `Request`: a new `Request` field breaks every literal |
| Logger levels (`warn`/`debug`) and structured fields | P (K5a #2586: `warn`/`debug`; experimental `log(level, msg, fields)` writing `{ message, level, fields }`) | L2 `Logger` | Workers Logs / Observability ingest JSON |
| Config / non-secret env vars (`[vars]`) | A | Existing `Secrets.get` (reads `env[name]`) | No new capability (D6). The gap is `[vars]` emission and deploy, which is deploy work |
| `Sagas`, `Tracer`, durable `Idempotency` | D | L2 | Design notes Tier 2. `Sagas` is post-1.0 language-adjacent (D8) |

### 6.2 Workers runtime APIs (globals, no binding)

| API | Status | Home | Notes |
|---|---|---|---|
| Fetch | P | L2 `Fetch` | §6.1 |
| Web Crypto | A | §6.1 | |
| Encoding (TextEncoder/Decoder) | P | L0 `Bytes` | |
| Web standards: URL, URLSearchParams, URLPattern, `structuredClone`, Intl, CompressionStream | A / P | L0/L1 (URL, compression); Intl → `Locale` | URLPattern is routing, so language-owned |
| Console | P | L2 `Logger` | |
| Performance and timers / Scheduler (`scheduler.wait`) | P (`Clock.now`) | L2 `Clock.sleep`? | Sleeping inside a DO handler interacts with input gates: decide before adding |
| Streams | P (`Stream[T]` for responses) | L0 | Byte streams are a prerequisite for R2 and large bodies |
| Cache API (`caches.default`) | A | L3 `bynk.cloudflare.Cache` | Cloudflare-specific semantics (per-colo, not global). Edge caching of a Worker's own responses, purgeable globally, is a different platform feature (Workers Cache) and shipped as `EdgeCache` (first-party registry L7, #2414) |
| HTMLRewriter | A | L3 | Streaming, callback-based: may fail R14.4. Late |
| TCP sockets (`connect()`) | A | L3 `Sockets` | Prerequisite for native DB drivers |
| EventSource | A | L3 or L2 | Low priority |
| WebSockets (outbound client) | A (inbound shipped via `Connection[F]`) | L2/L3 | |
| Headers / Request / Response | Language (inbound) + `Fetch` types (outbound) | n/a | |
| Context (`ctx.waitUntil`, `passThroughOnException`) | A | **Language/runtime**, §6.4 | |
| Handlers | Language, §6.4 | n/a | |
| RPC | Language (`consumes context`) | n/a | |
| MessageChannel, WebAssembly, Node.js compatibility | n/a | **Non-goal** (§14) | |

### 6.3 Workers bindings

Binding list as published by Cloudflare on 2026-10-14. "Instances" marks
bindings that are normally needed more than once per Worker, and so depend on
the resource-instance model (PR-2).

| Binding | Status | Home | Instances | Prerequisites / notes |
|---|---|---|---|---|
| Assets | S (language: `asset` declarations, #1883) | n/a | | |
| Durable Objects | S (language: `agent`) | n/a | | DO alarms are language, §6.4 |
| Service bindings | S (language: `consumes context`) | n/a | | Binding to a *non-Bynk* Worker: P, experimental `bynk.cloudflare` `Service` (interop track D4, I5 #1972) |
| Secrets | S (`bynk.Secrets`) | L2 | | |
| KV | P (`get`/`put`/`putTtl`/`delete`/`list` over text; several namespaces as derived capabilities, S4; P1 #2590, experimental: `getWith`/`putWith` with metadata, an expiry and `cacheTtl`, `getBytes`/`putBytes`, paged `listPage`) | L3 | **yes** | Bulk reads, streamed values (K-streams) |
| Environment variables | A | L2 | | §6.1 Config |
| Queues (producer) | P (shipped, experimental: `send`/`sendWith`/`sendBatch` over JSON text from `Json.encode`; instance-only `Queue @name("…")`; P2 #2578) | L3 `Queue` | yes | Consumer side shipped as protocol. A typed `send[T]`, and checking a producer's message against the consumer's, wait for a codec-carrying generic op |
| D1 | A | L3 `D1` | **yes** | D4 (typed rows), migrations story vs state-migrations gate, `Bytes` |
| R2 | P (shipped, experimental: `get`/`head`/`put`/`putWith`/`delete`/`list` over whole `Bytes` objects; instance-only; P5 #2323) | L3 `R2` | **yes** | Byte streams (split out of P5, see P5b), multipart, further conditions |
| Rate Limiting | P (shipped, experimental: `allow(key) -> Effect[Bool]`; instance-only `RateLimit @limit(n) @period(10.seconds \| 60.seconds)`; P3 #2342) | L3 `RateLimit` | **yes** | `namespace_id` allocated in the ledger, unique across its environments; a listing API to detect ids used outside Bynk |
| Analytics Engine | P (shipped, experimental: `write(DataPoint)`, limits checked before the write; instance-only `Analytics`, no options; dataset name derived in snake case; P3b #2582) | L3 `Analytics` | yes | `Bytes` blobs, and a typed `write[T]` mapping a record onto blobs and doubles |
| Analytics SQL binding | A | L3 | | Verify scope before admitting |
| Version metadata | A | L3 | | Trivial; observability |
| Secrets Store | A | L2 `Secrets` widened, or L3 | | Account-level secrets |
| mTLS | A | L3 (a `Fetch` variant) | yes | Security review |
| Hyperdrive | A | L3 | yes | Needs a Postgres/MySQL driver (`requires`) or TCP sockets; typed rows (D4) |
| Workflows | A | **Held** (D8) | | Candidate runtime substrate for post-1.0 sagas, not a user capability |
| AI, AI Search, Vectorize | A | L3 | | Model-specific payloads: D4 decides typed-only vs a JSON escape |
| Images, Media Transformations, Stream, Browser Run | A | L3, late | | Niche; Browser Run likely needs an npm driver |
| Email (`send_email`) | A (not in the bindings index; verify) | L3 | | Inbound email handler is language, §6.4 |
| Dispatcher (Workers for Platforms), Dynamic Worker Loaders | n/a | **Non-goal** | | Platform-builder features |

### 6.4 Routed out of this track (language or runtime work)

| Item | Why it isn't stdlib | Likely home |
|---|---|---|
| Inbound handlers: `email`, `tail`; DO `alarm` | Entry points are service protocols (`from …`), which is syntax | Protocol extension (ADR 0079 closed set) |
| `ctx.waitUntil` | Changes handler completion semantics | Runtime/language |
| `request.cf` (geo, colo, TLS) | Inbound request metadata | HTTP protocol surface |
| Workflows, sagas | Coordination layer, post-1.0 per the 1.0 definition | Coordination tracks |
| SQL checked at compile time | Compile-time-checked payload (R14.4, Tier 3) | Separate language track if wanted |

## 7. Prerequisites before any slicing

These must be in place, or explicitly decided against, before planning
stdlib slices.

- **PR-1 First-party unit registry.** Replace the hard-coded injection in
  `bynk-check/src/project_model.rs` (`phase_parse`: seven gated blocks with
  hand-written transitive cascades, e.g. map→list, locale→list/string/types)
  and the per-unit constants in `firstparty.rs` with one table. Each entry
  carries the unit name, kind, sources, intra-stdlib dependencies (closure
  computed, not hand-written), per-platform bindings, platform nativity,
  env-taking providers and wrangler derivation. Behaviour stays byte-identical
  (P-5). Without this, every new unit is a compiler change, which breaks ADR
  0034's promise.
- **PR-2 Resource-instance model.** *Design settled (D3):* derived
  capabilities, `capability Sessions = Kv`. What remains is building it:
  grammar, checker, kind-prefixed binding emission, and keying the deploy
  ledger by (environment, Worker, capability). Today there is exactly one
  `KV` per Worker (`firstparty.rs:82`), and it must stay byte-identical as
  the implicit default instance.
- **PR-3 Standalone binding test scaffold.** Each TS binding is type-checked
  and unit-tested on its own, against Miniflare where the semantics matter
  (R-12/R-13). This is ADR 0086's open follow-up.
- **PR-4 Stability marker** (R-17), so the first experimental item has
  somewhere to live. D2 makes this a 1.0 requirement, not just a convenience.
- **PR-5 Kernel gaps that block L3:** `Bytes` ops and byte streams (R2, crypto,
  binary Fetch), and the D4 decision on typed payloads (D1, AI, Hyperdrive).

## 8. Settled decisions

All nine were argued and settled in discussion with the maintainer on
2026-10-14, before this spine was opened. They are recorded here for
line-anchored review in the settling PR, which may reopen any of them. Each
load-bearing one lands as a front-loaded ADR with the slice that first
depends on it (§10), not with this doc. D7 and D8 are deferrals and need no
ADR.

- **D1 — Owning crate. SETTLED 2026-10-14: a new `bynk-std` crate.**
  - *What moves:* everything in `bynk-check/src/firstparty.rs` that is data,
    plus the `firstparty/` tree. That covers the `.bynk` sources, the TS
    bindings, unit names, the `Platform` enum, platform nativity
    (`platform_of`), env-taking providers (`provider_takes_env`), wrangler
    binding names, and `FIRSTPARTY_SOURCES`. It becomes the home of PR-1's
    registry table.
  - *What stays:* parsing and injection (`firstparty_parsed`, `phase_parse`)
    stay in `bynk-check` as checker logic that *reads* the registry.
    `runtime.ts` stays in `bynk-emit`, because it is codegen support, not
    stdlib.
  - *Layering:* the crate is a leaf, or depends only on `bynk-syntax` if the
    registry needs `UnitKind`. `bynk-check` depends on it, which is a
    downward extraction per ADR 0099. `bynk-check` re-exports `Platform`, so
    `bynkc`, `bynk-wasm` and `bynk-ide` call sites are unaffected.
  - *Why a crate and not a top-level `std/` directory:* `include_str!` from
    outside a crate's directory breaks `cargo publish`.
  - *Cost:* a 17th crate in `release.yml`'s publish order, placed before
    `bynk-check` and versioned in lockstep (`version.workspace = true`). The
    name `bynk-std` was unclaimed on crates.io on 2026-10-14.
  - *Must hold:* byte-identical emitted output (P-5); the crate builds for
    `wasm32`; and the `bynkc/tests/firstparty_sources.rs` guards move with
    the sources or point at the new crate.
- **D2 — The stdlib's 1.0 promise. SETTLED 2026-10-14: a tiered promise.**
  Context: Gate 1 of `bynk-1.0-definition.md` *does* address capabilities. It
  puts "the coordination and advanced layers — capabilities, …" outside the
  frozen layer, where they "evolve additively through 1.x". It leaves three
  things unsaid: the L0 kernel and L1 commons, which every Foundations program
  calls; what "additive" means for an operation that already exists; and what
  happens when the platform itself changes. The settled position:
  - **Stable L0 and L1 items join the Foundations layer.** A breaking change
    to one is a 2.0 event.
  - **Stable L2 and L3 items are additive-only through 1.x.** New items and
    operations may be added. A *variant* may be added only to an exported
    enum marked **extensible** (§11 Q5). The marker is first-party metadata,
    and a user `match` on such an enum must carry a wildcard arm, which a
    diagnostic enforces. Error enums (`FetchError`, and future `KvError` or
    `SqlError`) and growing vocabularies (`Method`) are marked extensible.
    Every other stable enum is closed: exhaustiveness is enforced
    (`bynk.types.non_exhaustive_match`), so a new variant is a major-release
    event. An existing stable item may be
    *deprecated* in a minor release (a warning on
    ADR 0117's channel, with a suggestion where one is mechanical) but
    *removed or reshaped* only in a major release.
  - **Experimental items carry no promise.** They may change or disappear in
    any release, and using one is visible to the user (R-17).
  - **Platform carve-out, L3 only:** a stable `bynk.cloudflare` item may break
    in a minor release *only* when Cloudflare withdraws or incompatibly
    changes the underlying feature. That release's cumulative migration note
    must name it. The carve-out never covers a Bynk-side redesign.
  - **Follow-through:** an amendment to `bynk-1.0-definition.md` Gate 1 adds
    the stdlib clause, and an ADR records the tiers. Both land with slice S3
    (§9). Before 1.0, nothing changes in practice: daily increments
    may still break anything, but stable/experimental marking (PR-4) must
    exist by the time 1.0 is cut.
- **D3 — Resource-instance model** (PR-2).
  - **Ownership: SETTLED 2026-10-14, context-owned.** KV, D1 and R2 instances
    belong to one context, keyed by **(environment, Worker, capability)** in
    the deploy ledger. The Worker is the context's deployment unit, and the
    environment is part of the key because ids differ per environment. The
    ledger's `kv` map, keyed by Worker today, gains the instance name, and the
    bare `Kv` maps to the existing entry. Other contexts reach the data only
    through that context's services. Queues stay account-named delivery
    channels (ADR 0122/0194). An instance's source name is part of its
    identity: renaming it plans a *create* plus an *orphan*, never a silent
    prune (`confirm_prune`).
  - **Syntax: SETTLED 2026-10-14, a derived capability with an implied
    platform provision.** The worked scenarios are in
    Appendix A (A1–A13).
    ```bynk
    context shop.cart
    consumes bynk.cloudflare { Kv, D1, Queue }

    capability Sessions = Kv
    capability Orders = D1
    capability OrderEvents = Queue @name("order-events")
    ```
    - **Meaning.** `capability X = K` declares a new capability `X` whose
      operations are those of platform capability `K`. Each declaration is a
      **distinct resource instance**, not an alias: two derivations of `Kv`
      are two namespaces. This is the meaning `=` already has in Bynk
      declarations: `type Id = Int` is a *distinct* nominal type over `Int`,
      not an alias (spec §6.1.2, §6.5). `capability Sessions = Kv` is a
      distinct instance over `Kv` in the same way (§11 Q1). Hover and the
      spec say "a distinct instance of `Kv`" and cite that parallel.
    - **Implied provision.** The user writes no `provides`. `X` is provided by
      `K`'s platform provider (e.g. `WorkersKv`), bound to `X`'s own binding,
      just as `consumes bynk.cloudflare { Kv }` gets `WorkersKv` today. This
      leaves `provides` with its one existing meaning (left: an existing
      capability; right: a new implementation name), which
      `provides Sessions = Kv` would have reversed (A13).
    - **Use.** `X` is an ordinary capability, so `given X`, `X.op(…)`,
      `stub X.op(_)`, `expect X.op called …`, deps-by-name and the platform
      lock all apply unchanged. It is declared once per context and visible
      across the context's files (#1820), so a misspelled reference is
      `bynk.given.unknown_capability`, not a silent second resource (A4).
    - **Options.** Creation parameters are `@annotations` on the declaration,
      mirroring `@ttl` on `store` fields: `@name("…")` (the account-shared
      queue name; `@queue`, the first spelling, does not parse, since `queue`
      is a keyword, P2 #2578), and later Vectorize dimensions/metric or R2 jurisdiction.
      An annotation outside the kind's closed set is a diagnostic. Changing
      one means re-creating the resource.
    - **`wrangler.toml` mapping (A12).** The compiler derives the binding name
      (kind-prefixed: `KV_SESSIONS`, `D1_ORDERS`, `QUEUE_ORDER_EVENTS`).
      `bynk deploy` derives the physical name from the Worker and the
      declaration, environment-qualified (`shop-cart-orders`,
      `shop-cart-orders-staging`). The ledger owns minted ids, keyed by
      (environment, Worker, capability). Ids never appear in source: adopting
      an existing resource is a per-environment deploy setting (planned
      `[deploy]` table, #551).
    - **Rules.**
      1. `K` must be a capability of an adapter the unit `consumes`, so the
         `consumes bynk.cloudflare` line stays the platform marker (ADR
         0016/0017).
      2. `K` must be an *instantiable* kind: KV, D1, R2, Queue producer,
         Vectorize, Analytics Engine, Hyperdrive. Workers AI (one `[ai]`
         binding) is not, and stays a plain consumed capability.
      3. `X` shares the unit's capability namespace and its conflict rules
         (`bynk.consumes.capability_name_clash`).
      4. A bare consumed `K` (`Kv`) remains the implicit default instance
         (binding `KV`), and it may be used alongside derived instances. Its
         stanza, like every instance's, is emitted and provisioned **only
         when the bare `K` is referenced** (R-7). Otherwise a context that
         consumes `Kv` only to derive `Sessions` would carry a permanent,
         unreachable `KV` namespace: a P-5 departure and an extra standing
         orphan/prune entry (§12). So every context that *uses* bare `Kv`
         emits byte-identically. A context that consumes `bynk.cloudflare`
         but never references `Kv` already emitted no `[[kv_namespaces]]`
         (the gate was use-based before S4a), so no output changed.
      5. Adapters may declare derived capabilities. Each consuming context
         gets its own instance (A6). Consumer-supplied instances are not
         offered for now; they could be added later without breaking
         anything.
      6. Platform-resource writes, including from agent handlers, commit as
         they return and are not rolled back on a fault: the ADR 0109 D4
         class of `~>` and `<-` (A11).
    - **Deferred.** An explicit `provides X = Impl { … }` over a derived shape
      (a user-written, unlocked implementation, A13). Type arguments on the
      derivation (`capability Sessions = Kv[Token, Cart]`) as a later upgrade
      to D4's two-step decode (A11).
- **D4 — Typed payloads for SQL and AI. SETTLED 2026-10-14: two-step decode.**
  ADR 0045 stands, and no untyped `Json` is introduced. Operations that return
  structured data (D1 and Hyperdrive rows, AI responses) return the whole
  result as **one JSON `String`**, which the caller decodes with
  `Json.decode[T]`. Typically that is `Json.decode[List[Row]]`, so a query
  needs one decode, not one per row. Inputs go the same way: `Json.encode`
  for AI inputs and queue messages, and a first-party closed sum
  (`SqlValue`) for SQL parameters. Why: a one-step `query[T]` needs the
  compiler to synthesise a decoder at the call site, which fails Part 14
  clause (iv); ADR 0281 shipped generic capability methods without a codec
  story. Known costs:
  - Bynk generics cannot wrap the decode, because a type variable is
    `json_uncodable`. Every call site writes its own `Json.decode`.
  - D1's slice must handle SQLite's 0/1 booleans and blob columns, which the
    JSON codecs don't represent.
  - A codec-carrying generic op (`Orders.all[Row](…)`) stays available as a
    later, Tier 2 ergonomic upgrade once a second consumer proves the shape.
- **D5 — Crypto is effectful. SETTLED 2026-10-14.** Hashing, HMAC,
  signing/verification and key import return `Effect`, implemented over the
  platform's Web Crypto (`crypto.subtle`), as the runtime's own verifiers in
  `auth.ts` already are. That means no third-party crypto code, the
  platform's audited implementation, constant-time verification and
  non-extractable keys. *Correction to the earlier recommendation:* a
  deterministic op cannot be made pure by hiding the async in lowering,
  because `Effect[T]` lowers to `Promise<T>` and pure functions lower
  synchronously (emission spec §150). A pure hash would mean shipping a
  synchronous implementation. Recorded as admission rule §4.2: pure only if
  synchronous on every platform without third-party code. *Cost:* no
  hashing inside pure functions, refinements or invariants. Encodings (hex,
  base64url, percent, URL) stay pure.
- **D6 — New capabilities are flat sibling units. SETTLED 2026-10-14.**
  Crypto ships as `bynk.crypto` (`consumes bynk.crypto { Crypto }`). ADR
  0012's `bynk` list stays closed, and each dependency is a greppable line.
  It is a Tier 0 unit with bindings for Cloudflare, Node (≥ 22, global
  `crypto.subtle`) and the browser, where the doc states the secure-context
  requirement (R14.5). Extensions to existing capabilities stay where they
  are: random bytes become `Random.bytes(n)`. *Config:* no new capability.
  `Secrets.get` already reads `env[name]`, which is where wrangler `[vars]`
  values appear, so the gap is `[vars]` emission and `bynk deploy` support.
  That is deploy work, tracked outside this track.
- **D7 — Time: UTC ISO-8601 now, time zones deferred. SETTLED 2026-10-14.**
  Pure L0 ISO-8601 parse and format for `Instant`, in UTC. A `TimeZone` type,
  DST semantics and calendar fields wait for demand, or for Temporal to be
  dependable on all three platforms. Cloudflare's web-standards page does not
  document `Intl` time-zone data.
- **D8 — Workflows: held for sagas. SETTLED 2026-10-14.** A Workflow is a
  `WorkflowEntrypoint` class in a Worker script, which Bynk cannot write
  without new language features. `terminate()` runs rollback handlers, which
  is compensation. Workflows are recorded as a **candidate runtime
  substrate** for the post-1.0 sagas track (Part 14 E7), not a
  `bynk.cloudflare` user capability. A trigger-only capability for
  TypeScript-authored Workflows in another Worker (`script_name`) is
  deliberately not offered.
- **D9 — Unbundling `bynk.cloudflare`: option kept open. SETTLED 2026-10-14.**
  The commitment is P-3 only: a later unbundling renames no unit and changes
  no user source. The structural hedge is that L3 sources and bindings
  live in their own subtree inside `bynk-std`, with no L0–L2 code depending
  on them. Whether to unbundle belongs to the packaging track.

## 9. Candidate slice decomposition

Provisional until this doc settles. Each slice becomes an increment proposal,
filed as a sub-issue of the spine. **Core and platform slices never share an
increment** (ADR 0023), and the ordering follows §13.

**Prerequisites** (no user-visible change except S3's marker):

| Slice | What | Settles / needs |
|---|---|---|
| S0 ✅ #1940 | Extract `bynk-std`: move the data in `firstparty.rs` and the `firstparty/` tree into a leaf crate; `bynk-check` re-exports `Platform`; output byte-identical; 17th crate in `release.yml`'s publish order | D1, D9 hedge (L3 in its own subtree) |
| S1 ✅ #1945 | First-party unit registry: one table (unit, kind, sources, dependencies with computed closure, per-platform bindings, nativity, env-taking providers, wrangler derivation) replaces the seven injection blocks; byte-identical. Done without wrangler derivation, which S4 reshapes (#1945 Decision E) | PR-1, R-6, P-5 |
| S2 ✅ #1952 | Standalone binding test scaffold: each TS binding `tsc --strict` on its own, behavioural tests against contract-faithful fakes, Miniflare where semantics matter | PR-3, R-12, R-13 |
| S3 ✅ #1963 | Stable/experimental marking (first-party metadata) with the `bynk.toml` `[unstable] allow` opt-in (§11 Q6); the extensible-enum marker and its wildcard-arm diagnostic (§11 Q5); the D2 ADR; and the `bynk-1.0-definition.md` Gate 1 amendment | PR-4, D2 |

**Instances:**

| Slice | What | Needs |
|---|---|---|
| S4a ✅ #1966 | Derived capabilities for KV, the language and build half: `capability X = Kv` grammar, checker rules 1–4 and 6 (D3; rule 5, adapter-declared instances, comes with the first adapter that needs one), kind-prefixed binding emission (`KV_SESSIONS`) under the per-Worker uniqueness check (§11 Q2), implied provision, per-instance test doubles; bare `Kv` byte-identical where it is referenced, and its stanza gated on reference (rule 4); `bynk deploy` and `dev --remote` refuse instances until S4b | S1, D3 |
| S4b ✅ #1967 | Derived capabilities for KV, the deploy half: ledger keyed by (environment, Worker, instance binding), one namespace provisioned per instance (`<worker>-<capability>`, environment-qualified), the deploy plan shows creates and orphans, `dev --remote` binds every instance; S4a's deploy guard lifted. S4 is complete | S4a |

**Core (L0–L2):**

| Slice | What | Needs |
|---|---|---|
| K1 ✅ #2016 | `Bytes` operations: `concat`, `slice` (clamped like `String.slice`), a `compare` method (`Bytes` stays unorderable), `toHex`/`fromHex` and `toBase64Url`/`fromBase64Url`, all L0 and none constant-time | — |
| K2 | URL type, query strings, percent-encoding | — |
| K3 | **Done** (#2592): `Instant.fromIso(s) -> Option[Instant]` (the RFC 3339 date-time profile, offsets converted, fractions truncated, calendar checked) and `toIso()` (`YYYY-MM-DDTHH:mm:ss.sssZ`), in the shared runtime | D7 |
| K4 ✅ #2327 | `bynk.crypto`: digests, HMAC, sign/verify, key import, `Effect` over Web Crypto; `Random.bytes`; security review. One `Crypto` capability in a flat sibling unit (Tier 0, `id_block` 7, experimental until the registry signs with it in production), a binding per platform (`Binding::PerPlatformFiles`), opaque non-storable `SigningKey`/`VerifyKey` (`bynk.types.key_at_boundary`), the Node binding as the unit-tier test double | K1, D5, D6, S2 |
| K5a | **Done** (#2586): `Method` gains `Patch`/`Head`/`Options`; `Fetch.sendWith(req, FetchOptions { timeout, redirect })` (experimental); `Logger.warn`/`debug`, and a structured `log` (experimental) | K1 |
| K5b | Fetch `Bytes` request and response bodies: two operations beside `send`, since `Request` cannot grow a field without breaking every literal | K5a |
| K6 | `Set` value type (K6a, #2596), Math (K6b, **done**, #2598: `Int` `div`/`rem`/`mod`/`pow` and the `Float` functions as value methods, `Float.pi()`/`Float.e()`), regex (K6c) | — |

**Platform (L3):**

| Slice | What | Needs |
|---|---|---|
| P1 | **Done** (#2590): complete KV, five experimental operations beside the unchanged ones: `getWith`/`putWith` (metadata, a `Never`/`After`/`At` expiry, `cacheTtl`; ADR 0051's options record), `getBytes`/`putBytes`, and a paged `listPage` (superseding ADR 0050's deferral); limits checked before the platform call | S4, K1 |
| P2 | **Done** (#2578): Queue producer, `capability X = Queue @name("…")` (not `@queue`: a keyword), `send`/`sendWith`/`sendBatch` over JSON text from `Json.encode`, sent parsed so a Bynk consumer decodes it; deploy creates a missing queue and records a producer-only one only when it created it | S4, D4 |
| P3 | **Done** (#2342), re-scoped: Rate Limiting (`@limit`, `@period`; ledger-allocated `namespace_id`, §11 Q7), with typed instance options. Analytics Engine split out (below) | S4 |
| P3b | **Done** (#2582): Analytics Engine, `capability X = Analytics` with no options (A12: deploy derives the dataset, `<worker>_<capability>` in snake case since it is a SQL table name), one `write(DataPoint)` with the platform's limits checked first; deploy only qualifies the name per environment, with nothing to create or record | S4 |
| P4 | D1: `SqlValue` parameters, two-step rows, the 0/1-Bool and blob codec gaps, the `migrations_dir` question | S4, K1, D4 |
| P5 | **Done** (#2323), re-scoped: R2 with whole `Bytes` objects, metadata, conditional ops, paged `list`, ledger-provisioned buckets. Byte streams split out (below); the §11 Q3 scope is treated as account-wide, since R2's docs state naming rules but not the scope | S4, K1 |
| K-streams | Byte streams: `Stream[Bytes]` across the boundary, a streamed `Raw` response. Language work (ADR 0023). Filed when a consumer needs objects too large to buffer | |
| P5b | R2 streamed `get`/`put`, multipart uploads | K-streams, P5 |
| L7 | **Done** (#2414), filed by the first-party registry track: `EdgeCache`, Workers Cache on per context (`[cache] enabled = true`), a prefix `purge`, and `no-store` on every `GET`/`HEAD` answer of an opted-in Worker but a `@cache` route's `2xx` | S4 |
| later | Cache API, Vectorize, Workers AI, Hyperdrive, TCP sockets, HTMLRewriter | case by case |

Outside this track: `[vars]` emission and deploy support (D6). It is deploy
work.

## 10. Front-loaded ADRs

The load-bearing, hard-to-reverse calls. Each lands with, or just before, the
slice that first depends on it. Numbers are assigned at merge.

1. **The stdlib's layers and admission rules.** L0–L3, §4's rules, including
   D5's purity rule ("pure only if synchronous on every platform without
   third-party code") and D6's flat-sibling-unit rule. Lands with S1.
2. **`bynk-std` owns the first-party sources.** D1, with D9's commitment and
   hedge. Lands with S0.
3. **The stdlib's 1.0 stability tiers.** D2, with the Gate 1 amendment, the
   `[unstable] allow` opt-in (§11 Q6) and the extensible-enum marker (§11 Q5).
   Lands with S3.
4. **Derived capabilities name context-owned platform resources.** D3:
   meaning, implied provision, `@annotation` options, the `wrangler.toml`
   three-name mapping, ownership and the six rules. Lands with S4a.
5. **Structured platform payloads decode in two steps.** D4. Lands with the
   first slice that returns a structured payload: P4, since P2 (#2578) only
   encodes.

## 11. Settling-review questions (all resolved)

Each question was raised in the settling review. All were closed on
2026-10-14: Q3 from the review evidence, and the rest with the maintainer.

1. **Derived-capability wording. Resolved: keep `=`.** In Bynk declarations
   `=` already introduces something distinct from its base: `type Id = Int` is
   a distinct nominal type, not an alias (spec §6.1.2: "a distinct named type
   over its base"; §6.5: "within a context, type identity is nominal").
   `capability Sessions = Kv` reads the same way. `:` ("an instance of a
   kind", as in `store live: Cache[…]`) was considered, but no other
   declaration uses `capability X: Y`. Hover and the spec use the wording "a
   distinct instance of `Kv`" (D3).
2. **Binding-name scheme. Resolved: kind prefixes, plus a per-Worker
   uniqueness check.** Prefixes alone cannot prevent collisions: service
   bindings are derived by uppercasing the dotted context name
   (`consumed_binding_name`: `kv.sessions` → `KV_SESSIONS`), and Durable
   Object bindings by screaming-snake-casing the agent (`agent_binding_name`:
   `KvSessions` → `KV_SESSIONS`). The collision is **already live**: a context
   that consumes `bynk.cloudflare { Kv }` and declares `agent Kv` emits two
   bindings named `KV` (one `[[kv_namespaces]]`, one
   `[[durable_objects.bindings]]`) with no diagnostic. This was reproduced
   with `bynkc` 0.314.23, and the derivation code on `main` is unchanged. It
   is being fixed outside this track as a general per-Worker binding-name
   uniqueness diagnostic. S4 depends on that check and extends it to derived
   capabilities. Existing names are not renamed.
3. **Physical-name derivation. Resolved in review.** The question's premise
   was wrong. R2 bucket names are scoped to the **account** (R2 addresses
   buckets under the account, `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`),
   not to all of Cloudflare. The derivation in A12 (Worker name plus
   declaration, `env_qualify`-ed) is therefore unique within the account
   whenever Worker names are, and Worker names already have to be unique per
   account. No prefix or hash is needed. P5 confirms the scope against R2's
   documentation before shipping.
4. **Provisioning. Resolved: keep the ledger.** Wrangler's automatic
   provisioning writes created ids *back into `wrangler.toml`*, but Bynk
   regenerates that file on every build, so the ids would be lost. Wrangler's
   documentation also does not say how it reuses an existing resource. The
   ledger already carries the CI gate for unrecorded namespaces and the
   `confirm_prune` gate, and ADR 0194's "the ledger owns the ids it mints"
   stands. Revisit if wrangler documents idempotent reuse by name.
5. **Adding a variant to a stable enum. Resolved: an extensible marker.**
   Exhaustiveness is enforced and the L2 surface exports enums transparently,
   so without a marker every stable error enum would be frozen for 1.x (the
   cost to R-1). First-party metadata (the ADR 0024 pattern, no syntax) marks
   selected exported enums as extensible. A user `match` on one must carry a
   wildcard arm, which a diagnostic enforces, so variants can be added in a
   minor release. The marker applies to error enums and growing vocabularies
   (`FetchError`, `Method`, and future `KvError`/`SqlError`). Other stable
   enums stay closed. Before 1.0 nothing is frozen, so K5 can add `Patch`,
   `Head` and `Options` to `Method` freely. The marker, and its one-time
   wildcard-arm migration for existing `match`es, lands with S3.
6. **Experimental-item UX. Resolved: an explicit opt-in in `bynk.toml`.**
   `[unstable] allow = ["<unit>.<Item>", …]`. An unlisted use is an error, and
   its suggestion adds the line. A warning per use was rejected because
   warnings cannot be silenced today. The new manifest table lands with S3
   (R-17).
7. **Rate Limiting configuration. Resolved: shape verified** on Cloudflare's
   Rate Limiting binding page (2026-10-14):
   `[[ratelimits]] name, namespace_id, [ratelimits.simple] limit, period`,
   where `period` is 10 or 60. The binding key is `name`, not `binding`.
   `namespace_id` is a positive-integer string *chosen by the user*, unique
   per account, and bindings sharing one share counters. So a derived
   `capability Login = RateLimit @limit(100) @period(60.seconds)` takes
   `limit` and `period` as annotations, and `bynk deploy` allocates
   `namespace_id` per (environment, Worker, capability) and records it in
   the ledger. Whether a collision with ids used outside Bynk can be detected
   depends on a listing API, which P3 checks.

## 12. Threat model

- **Crypto (`bynk.crypto`, K4).** Assets: key material (from `Secrets`) and
  signature validity. Adversary: a forged or replayed webhook or token, or a
  timing oracle. Mitigations: only the platform's Web Crypto (no third-party
  crypto, D5); constant-time `verify`; imported keys non-extractable by
  default; a security review for each crypto increment (R-16); and, in the
  browser, the secure-context requirement stated rather than polyfilled.
- **Provisioning and pruning (S4b, P1–P5).** Asset: production data in KV, D1
  and R2. Hazard: a renamed declaration or context changes the derived
  physical name, orphaning the old resource, and a prune deletes it. Mitigations:
  the deploy plan names every create and orphan; pruning stays behind
  `confirm_prune`; ids never live in source; adopting an existing resource is
  an explicit per-environment setting.
- **Context isolation (S4a).** Asset: a context's data. Instances are
  context-owned, a reference from another context is a static error, and
  access goes through that context's services. Adapter-declared instances are
  per consuming context, so a library never shares a consumer's data by
  accident.

## 13. Ordering criteria

Not a plan: these are the rules a plan should follow.

1. **Prerequisites first** (§7). PR-1 and PR-3 are refactors with no
   user-visible change, which makes them safe and independent.
2. **Unblockers before consumers.** A kernel gap (e.g. `Bytes` ops) comes
   before the L3 binding that needs it (R2).
3. **Never mix layers in one increment.** Platform work and stdlib work never
   share an increment (ADR 0023).
4. **Small, high-signal L3 items first** to prove the registry and the
   instance model: Queue producer, Rate Limiting, Analytics Engine, then KV
   completion. D1 and R2 come after D3/D4 and the `Bytes` work.
5. **Demand from real services wins ties.** The Fetch-header change (#1886)
   is the model: a concrete blocked service is the strongest evidence for a
   gap.

## 14. Non-goals

- A portable infrastructure tier (ADR 0016); portability is user-authored.
- Publishing `@bynk/*` packages beyond the emit ABI (ADR 0310).
- Third-party platform adapters or a user-facing platform marker (ADR 0024).
- Node.js compatibility (`nodejs_compat`), WebAssembly modules,
  MessageChannel, Workers for Platforms and Dynamic Worker Loaders.
- Full coverage of every Worker API before 1.0. The goal is a stated
  admission process and an honest inventory, not completeness.

## Appendix A — D3 worked examples

The reasoning behind D3's syntax: thirteen scenarios tried against three forms. Option C, as settled, is `capability X = K`.

### The three candidates

- **A. Alias on `consumes`:** `consumes bynk.cloudflare { Kv as Sessions }`.
  This extends the `as` that context consumption already has
  (`consumes commerce.shipping as Shipping`).
- **B. Resource handle values:** `resource sessions: Kv`, in the style of
  `asset`. A declaration yields a *value* that is "not data", and calls go
  through it: `sessions.get(k)`.
- **C. Declared instance capabilities:** `capability Sessions = Kv`. The
  declaration introduces a new **capability name** whose operations are the
  kind's operations. `Sessions` is a capability exactly as `Kv` is today: it
  appears in `given`, can be stubbed, and is wired by name through the deps
  object. It is not a value.

(A manifest table, the old option (b), drops out of A2: source still needs a
selector, so it becomes A plus a second place to keep in sync.)

Option C is shown throughout in its settled spelling, `capability X = K`
(A13). It was explored under a placeholder `resource` keyword, which B keeps.
"Held resource" already means `Connection[F]`, which was one reason to
drop it.

### A1 — Today's single KV (back-compat)

All three: unchanged. A bare `Kv` keeps meaning wrangler binding `KV`, so
`examples/link-shortener` emits byte-identically.

```bynk
context links
consumes bynk.cloudflare { Kv }

service api from http {
  on GET("/links/:code") (code: Slug) -> Effect[HttpResult[Url]] by Visitor given Kv {
    let stored <- Kv.get(keyOf(code))
    …
  }
}
```

### A2 — Two KV namespaces in one context

**A**
```bynk
context shop.cart
consumes bynk.cloudflare { Kv as Sessions, Kv as PriceCache }

service api from http {
  on GET("/cart") () -> Effect[HttpResult[Cart]] by Shopper given Sessions, PriceCache {
    let raw <- Sessions.get(shopperKey(by))
    let price <- PriceCache.get("sku:123")
    …
  }
}
```

**B**
```bynk
context shop.cart
consumes bynk.cloudflare { Kv }

resource sessions: Kv
resource priceCache: Kv

service api from http {
  on GET("/cart") () -> Effect[HttpResult[Cart]] by Shopper given Kv {   // ?
    let raw <- sessions.get(shopperKey(by))
    …
  }
}
```
B's first crack: what goes in `given`? `given Kv` no longer says *which*
store the handler touches, and `given sessions` puts a value where
capabilities go. Either the given list loses precision or it changes meaning.

**C**
```bynk
context shop.cart
consumes bynk.cloudflare { Kv }

---
Live cart sessions, keyed by shopper. Entries expire after 30 minutes idle.
---
capability Sessions = Kv

---
Read-through cache of catalogue prices; safe to lose.
---
capability PriceCache = Kv

service api from http {
  on GET("/cart") () -> Effect[HttpResult[Cart]] by Shopper given Sessions, PriceCache {
    let raw <- Sessions.get(shopperKey(by))
    let price <- PriceCache.get("sku:123")
    …
  }
}
```
Call sites are identical to A. The difference is that each instance has a
declaration, which gives it a doc comment, a hover target and a
go-to-definition target.

Wrangler output, all three: `KV_SESSIONS` and `KV_PRICE_CACHE`. Prefixing the
kind keeps these names clear of the service-binding (`OTHER_CONTEXT`) and DO
(`ORDER_ENTITY`) namespaces.

### A3 — D1 and R2 together, with D4's two-step decode

**C** (A differs only in the header; B in the handle spelling)
```bynk
context shop.orders
consumes bynk.cloudflare { D1, R2 }

capability Orders = D1
capability Invoices = R2

type OrderRow = { id: String, total: Int, paid: Bool }

service api from http {
  on GET("/orders") () -> Effect[HttpResult[List[OrderRow]]] by Shopper given Orders {
    let raw <- Orders.all("SELECT id, total, paid FROM orders WHERE shopper = ?1",
                          [SqlValue.Text(by.id)])?
    match Json.decode[List[OrderRow]](raw) {
      Ok(rows) => Ok(rows)
      Err(_)   => ServerError("order rows did not decode")
    }
  }

  on GET("/orders/:id/invoice") (id: String) -> Effect[HttpResult[()]] by Shopper given Invoices {
    match Invoices.get("invoices/\(id).pdf") {   // Effect[Option[Bytes]]
      …
    }
  }
}
```
This works the same under every option. It exposes D1's codec gaps, which
belong to the D1 slice, not to D3: `paid: Bool` arrives from SQLite as `0`/`1`,
and the `Bool` codec rejects that.

### A4 — A context split across files (#1820)

This is where the options separate. Today, **headers (`uses`, `consumes`) are
per file**, while **declarations are visible context-wide**. A capability
declared in `stamp.bynk` is `given` in sibling files without being
redeclared (`1820_multi_file_context_bundle_capabilities`).

**A**: each file that touches the namespace repeats the alias in its own
header:
```bynk
// shop/cart/read.bynk
context shop.cart
consumes bynk.cloudflare { Kv as Sessions }

// shop/cart/write.bynk
context shop.cart
consumes bynk.cloudflare { Kv as Session }   // typo
```
Under A, that typo **silently creates a second, empty namespace**: identity
is the spelling, scattered across headers. Reads miss writes in production,
and nothing fails until then. The checker could require every alias of a
kind to be spelled the same across a context's files, but that is a rule
invented to patch the form.

**B/C**: declared once, in any file, and visible to all of them:
```bynk
// shop/cart/resources.bynk
context shop.cart
consumes bynk.cloudflare { Kv }
capability Sessions = Kv

// shop/cart/write.bynk
context shop.cart
consumes bynk.cloudflare { Kv }
… given Session …        // bynk.given.unknown_capability (a "did you mean `Sessions`?" suggestion is plausible)
```
The typo is a compile error, because a misspelled *reference* has no
declaration to resolve to.

### A5 — A Bynk-written provider over an instance

A context wraps its namespace in a domain capability, which is today's
idiom (fixture 161).

**A / C**
```bynk
capability CartStore {
  fn load(shopper: String) -> Effect[Option[Cart]]
}

provides CartStore = KvCartStore given Sessions {
  fn load(shopper: String) -> Effect[Option[Cart]] {
    let raw <- Sessions.get("cart:\(shopper)")
    …
  }
}
```
**B**: a provider body would have to capture the `sessions` value. That
runs into ADR 0033 (closures over capabilities) and ADR 0031 (effects are
not storable) with a third kind of thing that is neither a capability nor
data. B needs a new rule here, and A and C need none.

### A6 — A library adapter that needs "a KV"

A rate limiter shipped as an adapter. Under context ownership, the cleanest
reading is that **the adapter declares its own instance, and each consuming
context gets its own copy**. The consumer never wires anything, and never
shares its own data with the library by accident.

**C**
```bynk
adapter myco.ratelimit {
  consumes bynk.cloudflare { Kv }
  exports capability { Limiter }

  ---
  Token buckets. One namespace per consuming context.
  ---
  capability Buckets = Kv

  capability Limiter { fn allow(key: String) -> Effect[Bool] }
  provides Limiter = KvLimiter given Buckets { … }
}

context shop.cart
consumes myco.ratelimit { Limiter }     // shop.cart now owns KV_MYCO_RATELIMIT_BUCKETS
```
**A** can express the same, with `consumes bynk.cloudflare { Kv as Buckets }`
in the adapter header. The same per-file repetition from A4 applies if an
adapter spans files. **B** needs the handle to cross into the consuming
context's composition, which again puts a value where a capability goes.

Open sub-question (all options): may a *consumer* hand the adapter one of
its own instances instead (`consumes myco.ratelimit { Limiter } given { Buckets = Sessions }`)?
Recommendation: not now. Per-adapter instances are the safe default, and
explicit wiring can be added later without breaking anything.

### A7 — Testing one instance, not another

**A / C**: the existing `stub` and `expect … called` forms, per instance:
```bynk
suite shop.cart {
  stub Sessions.get(_) returns Some("{\"items\":[]}")

  case "a cold price cache falls through to the catalogue" {
    stub PriceCache.get(_) returns None
    let r <- api.GET("/cart")
    expect PriceCache.put called once
    expect Sessions.put called never
  }
}
```
Each instance gets its own in-memory double, so `Sessions` and `PriceCache`
never see each other's keys. **B** works the same with lower-case names.

### A8 — Queue producer (account-named)

A queue's identity is an account-wide name, not the source name. The
instance therefore needs a payload: the queue it sends to.

**A**: the name has to be squeezed into the brace list:
```bynk
consumes bynk.cloudflare { Queue("order-events") as OrderEvents }
```
**C**: the declaration carries it:
```bynk
---
Order lifecycle events. Consumed by `fulfilment` (`service … from queue("order-events")`).
---
capability OrderEvents = Queue @name("order-events")

… given OrderEvents {
  let _ <- OrderEvents.send(Json.encode(placed))    // D4: encode is the caller's step
}
```
(Correction, A12: adopting an *existing* KV or D1 by id does **not** belong
on the declaration. Ids differ per environment, so they are deployment state,
not source.)

### A9 — Renaming (the deploy hazard)

All three: the source name is part of the resource's identity (context
ownership, settled). Renaming `Sessions` to `CartSessions` is therefore
*create a new namespace* plus *orphan the old one*. `bynk deploy` must show
this in the plan and must never prune without the existing confirmation
(`ledger.rs` `confirm_prune`). Ideally it also suggests the rename was
intentional. Under C, a hover could mention the identity rule on the
declaration itself. A rename that *keeps* the data needs the state-migrations
story (Gate 3) and is out of scope here.

### A10 — Another context reaching in

Context ownership makes this an error in all three:
```bynk
context billing
consumes shop.cart as Cart
… given Cart.Sessions …   // error: `Sessions` is a resource of `shop.cart`; reach it through a `shop.cart` service
```

### A11 — Compared with an agent's `store`

**Framing: these are separate concerns, not alternatives.** A `store` is
private to its agent: "only the owning agent's handlers can read it", and a
service reaches it only by calling one of the agent's handlers
(`bynk-design-notes.md` §478). Its mechanism (SQLite-backed Durable Object
storage, the staged overlay) is hidden. The declaration states only the
*semantics*: kind, key and atomicity. Bynk can hide the mechanism because Bynk
owns those semantics. D3 fills a different gap: platform resources reached
from a `service`. Their semantics are the platform's, so they are
deliberately *not* opaque (ADR 0016). The comparison below matters in just
two places: an **agent** handler that is also `given` a resource, where both
appear side by side, and the "which one should I use?" guidance.

`store` is Bynk's existing declaration of named, owned, persistent state, so
it is the natural yardstick:

```bynk
agent Sessions {
  key shard: String
  store live: Cache[Token, UserId] @ttl(30.minutes)

  on call login(token: Token, user: UserId) -> Effect[()] given Clock {
    do live.put(token, user)
  }
}
```

**What they share:**

| | `store` field | Context resource |
|---|---|---|
| Declared once, by name | ✓ | ✓ (B, C) |
| Owned by one unit | the agent (per key) | the context (settled) |
| Name is identity, so a rename loses data | ✓ (Gate 3 migrations) | ✓ (A9) |
| Per-declaration options | `@ttl(…)` | queue name |
| Typed values | `Cache[Token, UserId]`, codec generated by the compiler | two-step `Json.decode` (D4) |

On the surface, B *is* the store form: a lower-case name, declared in place,
called by name, with no `given`.

**Where they differ is in meaning, not syntax:**

| | `store` field | KV / D1 / R2 resource |
|---|---|---|
| Atomicity | Staged, flushed once at handler end; a fault persists **nothing** (ADR 0109 D1) | Each write lands when it returns; a later fault does **not** undo it |
| Read-your-writes | Always, through the overlay (0109 D2) | D1 yes; KV only per location, eventually consistent elsewhere (up to ~60 s) |
| Concurrency | Single writer: the agent's input gate serialises handlers per key | Any handler, on any isolate, concurrently; last write wins |
| Invariants | Checked against proposed state before the flush (0109 D3) | None possible: nothing is staged |
| Who can touch it | Only its own agent's handlers | Any handler in the context (`given`) |
| Portability | Portable; Bynk defines the semantics | Cloudflare-locked; the platform defines the semantics |
| In tests | Real in-memory state, driven through the agent | A double that a suite `stub`s and checks with `expect … called` |

ADR 0122 applied the same test when it ruled `Queue` out of the store
catalogue: *"Delivery is not storage; conflating them would put a runtime-side
concern behind the `store` surface."* KV, D1 and R2 fail the same test, so they
are not store kinds. A KV-backed `store` would be exactly the portable-
infrastructure lie that ADR 0016 rules out.

**What this means for the syntax choice:**

1. **Looking like `store` is a hazard, not a virtue.** Inside an agent, B puts
   two calls that read the same next to each other with different fault
   behaviour:
   ```bynk
   on call checkout() -> Effect[Result[(), CartError]] given Clock {
     do live.put(token, user)        // store: rolled back if the handler faults
     do sessions.put(k, v)           // B: KV, NOT rolled back
     …
   }
   ```
   Under C, the second line reads `do Sessions.put(k, v)` and the handler
   says `given Sessions`. A capitalised capability in the `given` list is how
   Bynk already marks "this reaches outside the agent's transaction": `Fetch`
   and `Clock` look the same way. That is the honest distinction.
2. **Agents may use resources, and the spec must say how.** ADR 0109 D4 rules
   per effect class on abort. Platform-resource writes join the
   "commits as it returns, not retracted" class, beside `~>` sends and
   cross-agent `<-` calls. That needs a sentence in the spec and in each
   capability's doc comment (R-2). Without it, a reader will assume store
   semantics.
3. **Borrow from `store` where the meaning matches:**
   - **`@annotation` for per-declaration options**, in place of `= "…"`:
     `capability OrderEvents = Queue @name("order-events")`, `@ttl` defaults
     for KV. (Not `@adopt`: see A12.) This is one option mechanism instead of a
     new one.
   - **Type arguments on the declaration** (`capability Sessions = Kv[Token, Cart]`)
     as a *later* upgrade to D4's two-step. The type is concrete at the
     declaration, so the codec would be generated once per resource, as for a
     `store` field, rather than at each call site. It is still compiler-
     generated, so under Part 14 it is Tier 2/3 work and stays out of the
     first slices. C leaves room for it, and A's brace form does not.
4. **Docs need a "which one?" page.** A `store` is for state that must be
   consistent and transactional per key. KV is for read-heavy, globally
   cached values that tolerate staleness. D1 is for relational queries
   across keys. R2 is for blobs. Agents and `store` stay the default state
   model, and resources are for the cases agents don't fit.

### A12 — What a qualified capability becomes in `wrangler.toml`

Every resource stanza in `wrangler.toml` involves up to **three different
names**, and each has a different owner:

1. **The binding name**: the property on the Worker's `env` (`binding = "…"`).
   It is code-facing only. **The compiler owns it** and derives it from the
   declaration, as it already does for `KV`, `ORDER_ENTITY` and
   `OTHER_CONTEXT`. The user never writes it.
2. **The physical name**: the resource's name on the Cloudflare account (a KV
   namespace title, `database_name`, `bucket_name`, `queue`, `index_name`,
   `dataset`). **`bynk deploy` owns it** and derives it from the Worker
   name and the declaration, environment-qualified the way queues and
   service targets already are (`env_qualify`: `<name>` for `default`,
   `<name>-<env>` otherwise).
3. **The id**: an opaque value Cloudflare mints at creation (KV `id`, D1
   `database_id`, Hyperdrive `id`). It is not knowable at compile time and
   differs per environment. **The deploy ledger owns it** (ADR 0194: "the
   ledger owns the ids it mints"). This is how KV works today: the emitter
   writes `id = "<KV_NAMESPACE_ID>"`, `bynk deploy` runs
   `wrangler kv namespace create`, records the id in the ledger, and
   materialises it into the config (`materialise_kv_namespace_id`).

So `capability Sessions = Kv` declares
**only the logical identity**: context + name + kind. Everything else is
derived:

```bynk
context shop.cart
consumes bynk.cloudflare { Kv, D1, R2, Queue }

capability Sessions = Kv
capability Orders = D1
capability Invoices = R2
capability OrderEvents = Queue @name("order-events")
```

```toml
# generated by the compiler; ids materialised by `bynk deploy` from the ledger
[[kv_namespaces]]
binding = "KV_SESSIONS"                   # 1. compiler
id = "<KV_SESSIONS_ID>"                   # 3. ledger; physical title "shop-cart-sessions" (2)

[[d1_databases]]
binding = "D1_ORDERS"                     # 1
database_name = "shop-cart-orders"        # 2. deploy-derived
database_id = "<D1_ORDERS_ID>"            # 3

[[r2_buckets]]
binding = "R2_INVOICES"                   # 1
bucket_name = "shop-cart-invoices"        # 2 (no id: the name is the identity)

[[queues.producers]]
binding = "QUEUE_ORDER_EVENTS"            # 1
queue = "order-events"                    # 2, but source-chosen: account-shared (ADR 0122/0194)
```

Per kind, from Cloudflare's configuration reference (2026-10-14):

| Kind | Table | Physical name | Minted id | Instances? | Notes |
|---|---|---|---|---|---|
| KV | `[[kv_namespaces]]` | title (not in the stanza) | `id` | yes | Shipped as the bare `KV` binding and derived instances (S4) |
| D1 | `[[d1_databases]]` | `database_name` | `database_id` | yes | `migrations_dir` is a D1-slice question: SQL schema migrations versus Gate 3 |
| R2 | `[[r2_buckets]]` | `bucket_name` (is the identity) | none | yes | Bucket names are unique per account, as Worker names are, so the derived name inherits Worker-name uniqueness (§11 Q3); optional `jurisdiction` |
| Queue producer | `[[queues.producers]]` | `queue` (is the identity, chosen in source) | none | yes | The only kind whose physical name the *source* picks |
| Vectorize | `[[vectorize]]` | `index_name` | none | yes | Index creation needs dimensions and metric, i.e. declaration options |
| Analytics Engine | `[[analytics_engine_datasets]]` | `dataset` (defaults to binding) | none | yes | Datasets appear on first write; no provisioning. Derived in snake case (`shop_cart_signups`, `…_staging`), not kebab, because it is the SQL API's table name (P3b #2582) |
| Hyperdrive | `[[hyperdrive]]` | config name | `id` | yes | Creation needs a database connection string, which is a *secret* input to deploy |
| Workers AI | `[ai]` | none | none | **no**: one per Worker | Stays a plain capability (`consumes bynk.cloudflare { Ai }`), with no derived capability |
| Rate limiting | `[[ratelimits]]` (key `name`, not `binding`) | none | `namespace_id`: an integer the *user* picks, unique per account; the ledger allocates it | yes | `limit`, and `period` of 10 or 60, are creation annotations (§11 Q7) |

**Consequences for the design:**

- **The declaration carries only what the source legitimately knows.** It
  never carries an id, because ids are per-environment deployment state. So
  "adopt an existing database" is a *deploy* concern. It belongs in a
  per-environment manifest table (the planned `[deploy]` table, #551),
  mapping `shop.cart.Orders` to an existing id, not in source.
- **Physical-name derivation is a public contract.** Renaming a context or a
  resource changes the physical name. That is A9's orphan hazard, now with a
  concrete mechanism: the ledger key is (environment, Worker, capability)
  (D3).
- **Options on the declaration are creation parameters, not identity.**
  `@name("…")` (account-shared name), Vectorize's dimensions and metric,
  R2's jurisdiction. Each is fixed for the resource's life, so changing one
  is also a re-create.
- **Wrangler can now auto-provision** KV, D1, R2 and Queues when the id is
  omitted. Whether `bynk deploy` keeps minting through its ledger or delegates
  this to wrangler is a deploy-slice question. ADR 0194's principle ("records
  nothing where another tool owns the state") argues for checking where
  wrangler records what it minted before choosing.

### A13 — `capability` or `provides` for the declaration?

`capability` declares a shape and `provides` declares an implementation.
`bynk.cloudflare` already has both halves for KV:

```bynk
capability Kv { fn get(key: String) -> Effect[Option[String]]  … }   // shape
provides Kv = WorkersKv                                              // implementation (TS binding)
```

Today's `provides` rules (spec §45, `diagnostics.rs`): the **left** side must
name an existing capability (`bynk.provider.unknown_capability`). The
**right** side introduces a *new* implementation name into the value
namespace. A capability is provided at most once
(`bynk.resolve.duplicate_provider`). A context's provider must have a body
(`bynk.context.external_provider`).

**`provides Sessions = Kv` reverses that direction.** The new name moves to
the left, and the existing name on the right is a capability, not an
implementation. Under today's rules it is an unknown capability (`Sessions`)
provided by an implementation called `Kv`. Giving `provides` a second,
opposite reading is the cost.

**Flipping it to keep the direction** (`provides Kv = Sessions`, `provides
Kv = PriceCache`) reads correctly ("Kv, provided by Sessions") but breaks
elsewhere. It needs `duplicate_provider` relaxed, and `Sessions` would be an
*implementation* name, which nothing can call or list in `given`: `given` and
the deps object are keyed by capability. Calls would still say `Kv.get`, and
the instance would be lost again.

**What the user actually declares is a shape-level fact:** "a new capability
named `Sessions`, with `Kv`'s shape". The implementation is not the user's to
write. It is the platform's provider, bound to that instance's own
namespace, just as `consumes bynk.cloudflare { Kv }` gets `WorkersKv`
without the user writing `provides`. That is why the derivation sits on
`capability`, with the provision implied:

```bynk
capability Sessions = Kv                            // shape: Kv's ops; provision: WorkersKv over KV_SESSIONS
capability Orders = D1
capability OrderEvents = Queue @name("order-events")
```

The two halves stay separable, so `provides` keeps its existing meaning if a
context ever wants its own implementation of a derived shape:

```bynk
capability Sessions = Kv
provides Sessions = MemorySessions { fn get(key: String) -> … { … } }   // user-written: no KV, no platform lock
```

ADR 0016 endorses exactly this kind of user-authored abstraction. It is not
needed for the first slices, but the form doesn't foreclose it.

Your instinct to put the queue name *inside* the provision
(`provides OrderEvents = Queue { @name(…) }`) is semantically right. The
queue name says *which resource implements* the capability, not what its
shape is. With the provision implied, the annotation has to sit on the
derived declaration, as `@ttl` does on a `store` field. That is the one place
the implied form blurs the layers. If an explicit, bodiless platform provision
were ever allowed in contexts, `@name` would move there.

### Scorecard

| | A alias | B handle values | C declared capability |
|---|---|---|---|
| A1 back-compat | ✓ | ✓ | ✓ |
| A2 `given` stays precise | ✓ | ✗ (or `given` changes meaning) | ✓ |
| A4 split contexts | ✗ silent duplicate on typo | ✓ | ✓ |
| A5 Bynk providers | ✓ | ✗ needs new closure/value rule | ✓ |
| A6 library adapters | ✓ (A4 caveat) | awkward | ✓ |
| A7 tests | ✓ | ✓ | ✓ |
| A8 queue name / future options | awkward | ✓ | ✓ |
| Doc comment, hover, go-to-definition site | ✗ | ✓ | ✓ |
| A11 reads differently from a transactional `store` | ✓ | ✗ looks identical, behaves differently | ✓ |
| Grammar cost | smallest (`as` in braces) | keyword + a new value kind | one contextual keyword (`asset` precedent) |
| ADRs touched | 0019 (selection form) | 0031, 0033, 0455-style "not data" | 0012/0016 untouched; capability model unchanged |

**Recommendation: C.** It keeps A's call surface and capability semantics,
where every existing rule (given, stubs, deps-by-name, the platform lock)
applies unchanged, and it gains B's declaration site, which is what fixes A4
and A8. As settled (A13), it reuses `capability`, so it needs no new keyword.

The settled rules for C are D3's six rules, in §8.
