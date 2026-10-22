# Packaging & distribution — the package unit, identity, resolution, and the registry protocol

- **Status:** Settled (#2199): every §11 question (Q1–Q10) was closed with
  the maintainer on 2026-10-20. Slicing is next. Spine
  [#843](https://github.com/bynk-lang/bynk/issues/843). The spine called this
  doc "a committed Draft", but #2199 is its first committed text. It started
  from the spine's settled agenda (Q1–Q11). It **re-opened and re-settled Q1**:
  the backbone is now a registry protocol, with each organisation bound to one
  registry. It also **dropped slice 0**, the flat-name cutover, in favour of
  `<alias>.<unit>`. §8 records each disposition, and §11 records each
  resolution.
- **Realises:** `design/bynk-design-notes.md` §3 (the bounded context as "a
  rough analogue of a module/package") and §19 (first-party capabilities under
  a root that depends on packaging). It also realises the deferrals in
  [ADR 0086](../decisions/0086-firstparty-sources-as-files-vendored.md) (the
  first-party publish, gated on the ABI freeze) and
  [ADR 0012](../decisions/0012-reserved-bynk-surface.md) (the ambient `bynk`
  surface "splittable later"). It closes the still-Open
  [ADR 0020](../decisions/0020-adapter-dependency-trust.md).
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md).
  It qualifies on all three axes (§1.2).
- **Governing records:**
  - ADR 0012: the reserved, flat-named `bynk` root.
  - ADR 0013: the explicit `binding … requires { … }` clause.
  - ADR 0019: adapters depend on adapters, and the closure walk is "the future
    lock-propagation walk".
  - ADR 0020: adapter npm-dependency trust (Open).
  - ADR 0024: platform nativity is first-party metadata.
  - ADR 0086: first-party sources are vendored, not published.
  - ADR 0200: the cross-context contract hash.
  - ADR 0255: deploy reconciliation, with the ledger keyed by Worker name.
  - ADR 0310: only the four-item emit ABI is published.
  - ADR 0463: `bynk-std` owns the first-party sources.
  - The standard-library track's P-1 (bundled and hermetic) and P-3 (an
    unbundling renames no unit).

## 1. The theme

### 1.1 What this track settles

Bynk code cannot cross a project boundary. Every project is an island: a
`bynk.toml`, a source tree, and the embedded first-party units. This track
adds four things:

- a **package unit**, which says what may be shared;
- an **identity model**, which says how a shared unit is named without
  colliding;
- a **dependency and resolution surface**, with a `bynk.lock`;
- a **distribution backbone**, which says where packages come from.

The spine's backbone was "a dedicated Bynk registry, npm for adapters, and
git/URL as the private hatch". This draft makes the backbone stronger in one
direction. **The registry is a protocol, not a service.** Bynk operates one
instance of it, the first-party registry. An organisation can run its own
private instance with the same client, the same lock and the same integrity
guarantees. Git and path dependencies remain, as a bootstrap and for local
work. They are no longer the only private option.

In short:

1. **A package is a project with an identity** (`org/package` in
   `bynk.toml`). It shares commons and adapters, and later behavioural units.
   It never shares contexts (spine Q2), so a dependency never becomes a Worker.
2. **Unit names in source do not change** (§3, re-settling slice 0).
   Dependency units are reached through a package-scoped prefix in the
   consumer's source. Nothing the consumer already has is renamed, so nothing
   already deployed is orphaned.
3. **Resolution is single-version.** `bynk.lock` pins each package's source
   and its **sha256** content hash (§5).
4. **Only the `bynk` driver touches the network.** It resolves the lock into
   an immutable local store and hands the compiler an in-memory source map.
   `bynkc`, the LSP and the playground stay offline and hermetic (§6.4).
5. **Each organisation resolves on exactly one registry**, from a
   per-project map. The first-party registry is the default (§6.2). This
   makes dependency confusion structurally impossible.
6. **The npm sub-layer stays npm.** It is folded and pinned, and conflicts
   become diagnostics (§7). ADR 0020 closes with an approve-and-diff step.

**End state when this track retires:**

- a consumer can depend on a Bynk package from the first-party registry, a
  private registry, git, or a path;
- the build is reproducible from `bynk.lock` with no network access;
- the dependency's adapter npm dependencies are reviewed before they enter the
  build;
- an organisation can stand up a conforming private registry from the
  protocol spec and the reference tooling.

The hosted first-party service is likely its own track (§9, slice 4b).

### 1.2 Why a track (the ADR 0076 trigger)

- **Multi-increment.** The track has eight ordered slices (§9) across
  `bynk-project`, `bynk-check`, `bynk-emit` and the `bynk` driver, plus a
  hosted service.
- **Surface not yet settled.** The identity model, the manifest tables, the
  lock format, and the registry protocol are all new surface. The spine's Q1
  and Q3 were re-opened and are re-settled here.
- **Security/safety boundary.** The track distributes code that the consumer
  compiles and ships, and it handles registry credentials (§12).

## 2. What's already true

Surveyed at v0.371.3 (2026-10-20); re-checked at v0.371.5 with no change to the surveyed files.

- **The manifest reserves the surface.** `bynk.toml` refuses unknown tables
  and keys (#1665, #1770). `PLANNED_TABLES` (`bynk-project/src/paths.rs:163`)
  rejects `dependencies`, `dev-dependencies` and `workspace` in any shape,
  with "planned but not yet supported (#843)". `[project]` accepts `name` and
  `version`, and both are display-only: nothing reads `name`.
- **Unit names are dotted, declared in source, and checked against the file
  path** (`unit_path_matches`, `paths.rs:758`). The `bynk` root is reserved
  (`bynk.namespace.reserved`). No org or package segment exists.
- **There is no cross-project source loading.** The only sources from outside
  the project are the first-party units. `bynk-std`'s `UNITS` table embeds
  them with `include_str!`, and `inject_firstparty` injects them on `uses` or
  `consumes`. The table is data-driven and organised to be unbundled later
  (ADR 0463; standard-library D9).
- **The compiler compiles an in-memory map.** It runs over
  `CompileOptions.sources: HashMap<PathBuf, String>`
  (`bynk-emit/src/project.rs`). The driver reads the disk
  (`bynk-driver/src/discovery.rs`).
- **The lockfile precedent: the caller pre-reads.** `SchemaLock::On {
  existing }` (`bynk-project/src/roots.rs:155`) carries `bynk.schema.lock`'s
  content, which the caller pre-reads. `bynk-emit` never reads it from disk,
  and `None` means "verified absent". Two other lock/pin files already sit
  beside `bynk.toml`: `bynk.deploy.lock` (the deploy ledger) and
  `bynk.foreign.toml` (foreign-caller contract-hash pins).
- **Deploy names come from context names alone.** The ledger
  (`bynk/src/deploy/ledger.rs`) keys Workers, KV namespaces and derived
  instances by Worker name. The deployed name is
  `deployed_worker_name(environment, worker)`
  (`bynk/src/deploy/config.rs:487`), which is the context name with dots
  dashed (`worker_dir_name`, `paths.rs:699`) and the environment appended.
  The project name plays no part.
- **npm `requires` are folded naively.** `binding "…" requires { "pkg":
  "range" }` (ADR 0013) is collected project-wide into a
  `BTreeMap<pkg, range>` (`bynk-check/src/project_model.rs:1150-1175`).
  Unpinned ranges are refused there. **Two adapters that require different
  ranges of the same package are not diagnosed: the last insert wins.** The
  map becomes the single root `out/package.json`.
- **There is no network code.** No HTTP or git crate is a dependency. The
  only outbound traffic is pinned `npx --yes` for Node tooling and `bynk test
  --against`.
- **There is no integrity hashing.** Contract hashes are FNV-1a 64 (ADR
  0200). They are fine for detecting skew, but they are not a defence against
  an adversary. Nothing is content-addressed.
- **Two first-party npm packages are designed but unpublished.** ADR 0310's
  emit-ABI package is not built. `@bynk/wire-client` (`bynk-emit/wire-client`)
  is the typed client's runtime, imported by name. It was decided in the
  interop track (D3, Q6–Q7, recorded in `archive/retired-tracks.md`). The
  track's ADR plan assigned it **no ADR**. **It is not published to npm**:
  the docs say to depend on it by file path or git. ADR 0310 names "the
  `@bynk/*` publication mechanics" as packaging-track work, so this track owns
  both (§7.3).
- **Open issues already wait on this track:**
  - #606: where the binding npm-dependency surface lives;
  - #911: message codes are unnamespaced and wait on the identity model.

## 3. The package unit and its identity

### 3.1 What a package is (spine Q2, kept)

A package is a Bynk project with a `[package]` identity. It may export
**commons** and **adapters** (with their bindings and `requires`). It may
later export **behavioural library units**, which are slice 6 and gated on
the agent-capability-encapsulation track. **A context is never shareable.** A
context is a deployment unit, and its Worker, its secrets and its ledger
entries belong to the project that deploys it. This one rule does most of the
work in §3.3: **no dependency ever produces a Worker.**

### 3.2 Identity: `org/package` in the manifest

```toml
[package]
name = "acme/money"      # organisation [a-z][a-z0-9-]* / package [a-z][a-z0-9_]*
version = "1.4.0"        # semver; required to publish
```

**Segment grammar (slice 1, #2207 decision G).** The two segments differ
because only one of them reaches source. The **package** segment matches
`[a-z][a-z0-9_]*` and isn't a Bynk keyword: an alias defaults to it (§3.3) and
is written as `<alias>.<unit>`, where `-` would lex as minus. The
**organisation** segment matches `[a-z][a-z0-9-]*`, since it appears only in
manifests, the lock and registry URLs. Each is at most 64 characters, and the
organisation `bynk` is reserved.

The organisation is the unit of **ownership** (who may publish) and of
**routing** (which registry, §6.2). The package is the unit of **versioning
and resolution**. A **unit** keeps the dotted name it declares in source, and
that name is unique **within its package**.

**A departure from the spine.** The spine lists `workspace` as an identity
tier (`organisation`/`workspace`/`package`/`unit`). This draft makes a
workspace a **build grouping only**: members, a shared root lock and shared
`[registries]` (§4.2). It does not appear in any name. A package's identity
must survive moving it between repositories, and a workspace is a fact about
a repository. Cargo and npm workspaces work the same way. Settled in §11 Q3.

### 3.3 Re-settling slice 0: no flat-name cutover

The spine's slice 0 is a breaking, codemod-driven rename of every unit to
`organisation.package.unit`. This draft proposes **not doing it**:

- **Its only victims are already deployed.** A dependency never produces a
  Worker (§3.1), so whatever a dependency's units are called never reaches
  the ledger. The cutover renames the **consumer's own** units, and the
  consumer's own contexts *are* Workers. `deployed_worker_name` and every
  ledger key are derived from those names (§2). A rename therefore makes
  every deployed Worker look new and every old Worker and KV namespace look
  orphaned. `--prune` would then offer to delete KV namespaces holding
  production data. The deploy track's retirement summary named this risk
  ("a rename orphans already-live resources with no automatic recovery").
  The cutover would make it real for every user at once.
- **It conflicts with the standard library's commitment.** Standard-library
  P-3 says that unbundling renames no unit (`bynk.cloudflare` stays
  `bynk.cloudflare`). The ADR 0012 reserved root already gives first-party
  units the collision-proof namespace the cutover was meant to buy.
- **It buys nothing that package scoping does not.** Collision safety needs
  names to be unique within a package, plus a prefix where the consumer
  refers to another package's unit. It does not need every name to carry its
  organisation.

**Proposed rule (D2).** A consumer refers to a dependency's unit as
`<alias>.<unit>`. The alias is the dependency's key in `[dependencies]`, and
it defaults to the package segment of its name:

```toml
[dependencies]
money = { package = "acme/money", version = "^1.4" }
fx    = { package = "globex/money", version = "^2" }   # same package name, other org: alias it
```

```bynk
context shop.checkout {
  uses money.currency     // acme/money's `currency` commons
  uses fx.rates           // globex/money's `rates` commons
  ...
}
```

Within its own source, a dependency still writes `uses currency`. The
compiler resolves names per package, so the same unit text means the same
thing in both places. The checker enforces four rules:

- **An alias may not be `bynk`.** It is reserved by ADR 0012, and first-party
  units keep being injected under their full names, unchanged.
- **An alias may not equal the first segment of one of the consumer's own
  units.** That would be ambiguous, so it is a diagnostic rather than a
  shadowing rule.
- **A dependency's unit is visible only through its alias.** It is never
  visible bare. Adding a dependency therefore never changes what an existing
  name means.
- **Only units the dependency's manifest lists in `exports`, or that the
  shareable-unit set admits, are reachable** (§12).

**The first-party surface is the one special case.** It keeps its reserved
full names (`bynk`, `bynk.cloudflare`, …). It is not an alias: today it is
an implicit dependency on the toolchain itself, and after slice 7 it is an
implicit dependency on the reserved `bynk` org. Both paths keep P-3. Settled
in §11 Q2.

**Emitted output.** A dependency's units emit under
`out/deps/<org>/<package>/…`, so the generated TS never collides even when
two packages declare the same unit name.

**Had review kept the cutover** (it did not, §11 Q2), slice 0 would also
have had to deliver a **deploy-ledger migration**. That migration is real
work, recorded here for any future reconsideration:

- a ledger v2 with an explicit `renamed: { old → new }` map;
- a `bynk deploy` plan that shows renames as *moves*, not create plus orphan;
- KV ids re-keyed in place (namespaces are account resources addressed by
  id, so they survive);
- Workers recreated under the new names, with routes and service bindings
  moved and the old Worker reported but never auto-deleted;
- Durable Object classes renamed through wrangler `renamed_classes`
  migrations, which is the state-migrations track's territory (§13).

### 3.4 Where a qualified name *could* reach deployed state

Slice 6 (behavioural library units) is the one place this changes. A
dependency's agent, mixed into a consumer's context, becomes a Durable Object
class in the consumer's Worker, and its class name is persisted deployed
state. Slice 6's own ADR must fix that naming, and it must coordinate with
#2188, which is making agent-class renames deliberate.

## 4. The manifest and workspaces

### 4.1 Tables

| Table | Keys | Notes |
|---|---|---|
| `[package]` | `name`, `version`, `description`, `license`, `repository`, `exports` | Present only in a shareable project. `exports` lists the reachable units; absent means every commons and adapter. |
| `[dependencies]` | `<alias> = { package, version }` · `{ package, git, rev }` · `{ package, path }` | Always a table; there is no bare-string shorthand (§11 Q5). |
| `[dev-dependencies]` | as above | Visible to `test` sources only. |
| `[registries.<name>]` | `index`, `organisations`, `mirror-of`, `key` | §6.2. Credentials are **never** here. |
| `[workspace]` | `members`, `exclude` | §4.2. |

`[project]` stays for applications, and its `name` stays display-only. A
project with both tables is a refusal (`bynk.manifest.project_and_package`),
so a project never has two names to disagree on. Settled in §11 Q4.

Slice 1 (#2207) moved `workspace` from `PLANNED_TABLES` into
`MANIFEST_TABLES`, along with `package`, and added them to
`manifest_schema.rs`. The unknown-key and did-you-mean behaviour did **not**
come for free for the other three: `[dependencies]`, `[dev-dependencies]` and
`[registries]` are **map tables**, whose keys are names the user chooses
(aliases, registry names) and whose entries have fixed keys. They live in a
second list, `MANIFEST_MAP_TABLES` (table → entry keys): each map key is
checked against its grammar (§3.2), then each entry's keys with did-you-mean.
The schema describes them as objects whose `additionalProperties` is the
closed entry schema.

### 4.2 Workspaces

A root `bynk.toml` with `[workspace] members = ["packages/*"]` holds:

- **one `bynk.lock`** for every member;
- **the only `[registries]`**: a member's own `[registries]` is a refusal;
- the path dependencies between members, which resolve without a version
  check.

A workspace has no identity (§3.2), and its members publish independently.

## 5. Resolution and `bynk.lock`

### 5.1 Single-version resolution (spine Q6, kept)

Each package appears **at most once** in a resolved graph. The resolver
picks the highest version that satisfies every caret range in the graph. If
no version does, that is `bynk.package.version_conflict`, naming both
requirers.

Allowing two versions of one package would let two copies of a commons's
nominal types coexist, so `money.Money` from 1.x and from 2.x would be
different types with the same name. That is the confusion single-version
resolution exists to rule out.

### 5.2 The lock (spine Q7, settled here)

```toml
# bynk.lock — generated by `bynk`; commit it.
version = 1

[[package]]
name = "acme/money"
version = "1.4.2"
source = "registry+bynk"                       # the first-party registry
checksum = "sha256-3q2+7w…"
dependencies = []

[[package]]
name = "acme/ledger-kit"
version = "0.3.0"
source = "git+https://git.acme.internal/ledger-kit?rev=9f1c2e0"
checksum = "sha256-Ld8x…"                      # of the canonical tree at that commit
dependencies = ["acme/money"]

[[npm]]                                        # slice 3/5: the npm sub-layer
package = "jose"
range = "^5.2.0"
required-by = ["acme/ledger-kit:auth"]         # package:adapter
approved = true                                # slice 5 (ADR 0020 closure)
```

The lock records three things for each package:

- **its source.** A registry source records the **registry name**, not the
  URL, so a mirror swap does not rewrite the lock.
- **its sha256.** This is a new `sha2` dependency, admitted through
  `deny.toml`. FNV-1a stays for contract hashes, which are not adversarial
  and are a different job.
- **its resolved dependencies.**

The lock also has one `[[registry]]` row per registry used, holding its
key fingerprint (§6.2). This row is what makes a private registry's
trust-on-first-use stick across fresh clones.

Two modes use the lock:

- `bynk check --locked` refuses if resolution would change the lock. This
  mode is new: `bynk.schema.lock` is a precedent for the file and its
  "verified absent" state, not for the flag.
- `bynk check --offline` refuses any fetch.

## 6. The distribution backbone (Q1, re-settled)

**Re-settled (§11 Q1).** The spine marked Q1 decided: a dedicated registry,
npm for adapters, and git/URL as the bootstrap and the private hatch. This
section
keeps all three and changes one thing. **Private registries are first-class
instances of the same protocol as the first-party registry.** Git becomes a
convenience rather than the only private route.

### 6.1 The registry protocol

The protocol is deliberately small, so that a private registry can be a
static file server or an object-store bucket:

- **Config.** `GET <index>/config.json` returns `{ "protocol": 1, "dl":
  "<archive base>", "api": "<publish base, optional>", "auth": "none" |
  "token", "id": "<registry id>", "keys": [ … ] }`. `id` is the registry's
  stable identity, and the first-party registry's is `bynk`. `keys` is the
  signed key-rotation record (D12, §11 Q10). Each rotation entry is signed
  over its own `id`, so a rotation record cannot be replayed from another
  registry.
- **Index (sparse, static).** `GET <index>/<org>/<package>` returns one JSON
  line per version. Each line holds:
  - `version` and `sha256`;
  - `deps`, each `{ package, range }`;
  - `npm`, the adapter `requires` it would fold;
  - `toolchain`, the minimum `bynk` version;
  - `yanked`;
  - `sig`, the origin's ed25519 signature (D12, §6.2).

  The signature covers a **canonical encoding** (*made exact by slice 4a:*
  JSON with keys in byte-wise order and no insignificant whitespace) of the
  following fields:
  - the origin registry's `id`;
  - the package's full `name` (`org/package`);
  - every field above except `yanked`.

  The name normally appears only in the URL path, so it has to be inside
  the signature. The client checks that the signed `id` is the registry it
  routed the org to (§6.2). It also checks that the signed `name` is the
  package it asked for. Without these two checks, a mirror could serve a
  validly signed line and archive from `acme/harmless` under the name
  `acme/payments`.

  It is append-only. A version line is never edited, except to flip
  `yanked`. The signature does not cover `yanked`, so flipping it never
  re-signs the line. A mirror that lies about yank state can only hide
  or invent a yank. It cannot change the content.
- **Archives (immutable, content-addressed).** `GET <dl>/sha256/<hex>`
  returns the package's files. *Amended by slice 4a (#2274, decision B):* the
  signed checksum is the **canonical-tree** sha256 the 2b store already uses,
  and the archive is transport only — `bynk-tree-v1\n` followed by each file as
  `path \0 length \0 bytes`, the very stream that checksum hashes, so no tar
  (and no links, modes or traversal) is involved. The client unpacks plain
  relative paths only and admits the tree when its checksum is the signed one.
  **Yanking never deletes.** A locked build of a yanked version still fetches
  it.
- **Publish (optional API).** `PUT <api>/<org>/<package>/<version>` with the
  archive as the body and a bearer token, and `POST …/yank` or `…/unyank`. The
  **registry signs** what it admits, so publishers hold tokens, never the key,
  and `bynk publish` verifies the signed line by reading it back. `api` obeys
  `dl`'s scheme rule. A static registry omits `api` and is published to by
  `bynk registry add`, which writes the index and archive files into a
  directory; `bynk registry serve` puts the API in front of such a directory
  (slice 4c, #2287).
- **Auth.** Bearer tokens come from `BYNK_REGISTRY_<NAME>_TOKEN` (the
  registry's name uppercased with `-` as `_`; a registry name matches
  `[a-z][a-z0-9-]*` and holds no `_`, so no two registries share a variable,
  and `bynk` is never a registry name) or the user
  credentials file (`~/.config/bynk/credentials.toml`), and never from the
  manifest or the lock. A token is sent **only to its registry's own
  origin**. A redirect to another origin drops it.

The first-party registry is one deployment of this protocol, with an `api`,
org ownership, and its own account policy (§9, slice 4b).

### 6.2 Organisations bind to registries (npm-scope style)

```toml
[registries.acme-internal]
index = "https://bynk.acme.internal/index"
organisations = ["acme", "acme-labs"]

[registries.corp-mirror]
index = "https://mirror.example.internal/bynk"
mirror-of = "bynk"            # fetch first-party packages through here
```

The binding has three rules:

- **An organisation resolves on exactly one registry.** It is the one whose
  `organisations` lists it, or the first-party registry if none does. There
  is **no per-dependency registry override**. The reserved org `bynk` can
  never be rebound (`bynk.registry.reserved_org`).
- **The map is per project, or per workspace.** A dependency's own
  `[registries]` is ignored when it is consumed. Only the root's map routes.
- **A mirror serves another registry's content and adds no names of its
  own.** Mirrors are **one level only**: `mirror-of` names `bynk` or a
  registry that isn't itself a mirror, so chains and cycles are refused
  (slice 1, #2207). For an **already-locked** version, the lock pins the hash, so a
  compromised mirror can withhold the archive but cannot substitute it.
  **First resolution is different.** A fresh `bynk add` or a lock update
  verifies the archive against the index, and with `mirror-of` that index
  comes from the mirror. A hostile mirror could serve a forged index line
  together with a matching forged archive. The signed index closes that gap.
- **The index is origin-signed (D12).** Each version line carries an
  ed25519 signature by its origin registry. A mirror relays the signed lines
  and cannot mint its own. The client verifies the signature against the
  origin's pinned key before it trusts the line's sha256:
  - the first-party key ships in the `bynk` binary;
  - a private registry's key is pinned in `[registries.<name>] key =
    "ed25519:…"`.

  Without a pinned key, a private registry is trust-on-first-use, and the
  client says so. **The first-seen key is persisted.** `bynk.lock` records
  a fingerprint for each registry it used:

  ```toml
  [[registry]]
  name = "acme-internal"
  key = "ed25519-fp:…"
  ```

  On every later resolution, a key that differs from the locked one is a
  hard error (`bynk.registry.key_mismatch`). The only exception is a key
  that a valid rotation chain from the locked key reaches. TOFU therefore
  means trust on *first* use, not trust on every fresh clone.

  **Key rotation** works through a record in `config.json`. Each new key is
  signed by the key it replaces, and the client follows that chain from
  its pinned or locked key. **Rotation covers planned rotation only.**
  Whoever holds a leaked old key can sign a chain to a key they control,
  and every client pinned to the old key would follow it. Recovering from a
  compromised key therefore needs a deliberate out-of-band step:
  - for the first-party key, a toolchain release that pins the new key and
    refuses the compromised one;
  - for a private registry, a `[registries]` key edit and a lock update.

  Revocation and threshold roles are the TUF depth deferred in §11 Q10.
  For locked versions, the lock already defeats rollback.
- **The lock's `source` is authoritative against routing.** Suppose a
  locked package's recorded `source` differs from the registry the current
  map would route its org to. For example, a clone or a standalone
  workspace member is missing the `[registries]` entry, so `acme` now falls
  through to the first-party registry. That is a **hard error in every
  mode** (`bynk.registry.source_mismatch`), not only under `--locked`. A
  misconfigured route therefore fails instead of silently resolving a
  same-named organisation elsewhere. An unlocked package has no record to
  check against, which is why the first-party registry also **claim-blocks
  organisation names on request** (slice 4b policy): a private
  organisation can reserve its name publicly without publishing there.

**The publication rule that closes the loop.** The first-party registry
refuses a package whose dependencies would resolve outside it. A private
registry may accept dependencies on first-party packages. Public packages
therefore never depend on something a public consumer cannot fetch.

**The alternative not recommended: the registry is orthogonal to identity
(Cargo style).** Each dependency names its registry, and the lock carries it.
This is more flexible, but the same `org/package` could resolve from two
places in one graph. That is exactly the dependency-confusion surface the
org binding removes. The org binding also gives private organisations a
natural home: an `acme` organisation is private because it is bound to
acme's registry.

### 6.3 Git and path dependencies (kept)

- **git.** `{ package, git, rev }`. The lock records the commit and the
  sha256 of the canonical tree. The fetch shells out to the user's `git`
  binary, following the pinned-`npx` precedent rather than adding a git
  crate (§11 Q7).

  The URL comes from a manifest, possibly a transitive one, so it is
  untrusted input. **Slice 2 must harden the invocation.** Otherwise `git`'s
  `ext::` transport or an option-shaped argument would run commands at
  resolve time, which would break §12's "resolving executes nothing". The
  minimum is:
  - `-c protocol.allow=never` with only `https` and `ssh` re-allowed for
    a git URL from a fetched (non-root) manifest. `file` is allowed only in
    the root manifest and in workspace members, and it is under the same
    confinement as path dependencies below. A transitive
    `git = "file:///…"` is refused (`bynk.dependency.path_outside_root`),
    because otherwise it would pull a local repository from outside the
    project into the graph;
  - `--` before every positional argument, and a refusal of any URL or rev
    that starts with `-`;
  - no submodule recursion;
  - `-c core.hooksPath=/dev/null`;
  - cloning straight into the store, never into the project tree.
- **path.** `{ package, path }`. This is for workspaces and local
  development. It is never locked by hash, because it is the user's own
  working copy.

  **Path dependencies are honoured only in the root manifest and among
  workspace members.** A path dependency declared by a fetched package is
  refused (`bynk.dependency.path_outside_root`). That covers a git
  dependency's `bynk.toml` saying `{ path = "../../.." }`, which publishing
  would refuse but git dependencies never pass through. A resolved path must
  also stay inside the workspace root once symlinks are followed. Otherwise
  a fetched tree could make resolution read sources from outside itself,
  and the lock would not see it.
- **Neither can be published.** A registry refuses a package with a git or
  path dependency, as crates.io does.

### 6.4 The network boundary: only `bynk` fetches

This follows `SchemaLock`'s rule that the caller pre-reads and the compiler
never touches the disk:

1. `bynk` (`check`, `build`, `dev`, `test`, `deploy`, plus new `add`,
   `fetch`, `publish`) resolves `bynk.toml` against `bynk.lock`. It fetches
   any missing archive into the **store**,
   `~/.cache/bynk/store/sha256/<hex>/`, where entries are immutable,
   read-only, and verified on insert.
2. The driver adds each package's sources to `CompileOptions.sources` under a
   package-qualified synthetic path, along with each package's identity.
3. **`bynkc`, `bynk-lsp` and `bynk-wasm` never fetch.**
   - `bynkc compile` reads a lock-resolved store, or `--vendor <dir>`.
   - The LSP reads the store. If an archive is missing, it reports a
     diagnostic suggesting `bynk fetch`.
   - The playground has no dependencies.

   A missing archive is an error, never a fetch.
4. `bynk vendor` copies the locked archives into `vendor/` for CI that has
   no network.

This keeps standard-library P-1 ("no network fetch … builds stay hermetic")
true of the compiler itself. The fetch is a separate, explicit, auditable
step.

## 7. The npm sub-layer

### 7.1 Propagation (slice 3)

A dependency's adapter `requires` fold into the consumer's root
`out/package.json` along the ADR 0019 closure walk. Two changes come first:

- **Conflicting ranges become a diagnostic** (`bynk.requires.conflicting_range`,
  naming both adapters) instead of last-insert-wins. This defect exists today
  within a single project, so it should be fixed as its own bug ahead of this
  track. Range intersection can come later.
- Each folded requirement records `required-by` in the lock.

### 7.2 Trust (slice 5, closes ADR 0020)

- `bynk add`, `fetch` and `update` list each **new or changed** npm
  requirement with its `required-by` chain, and ask for approval on a
  terminal; `bynk approve` is the one way to approve (slice 5, #2296).
- Approvals are recorded in the lock (`approved = true`), per
  `(package, range)`. A row only path packages require needs no approval.
- **The compile refuses** a reached fetched-package adapter's unapproved
  requirement (`bynk.requires.unapproved`), with or without `--locked`, so
  `bynk`, `bynkc` and the LSP agree (amended by #2296 decision B; this
  originally read "`bynk check --locked` refuses").
- Integrity of the npm tarballs themselves stays with npm's own lockfile.
  Bynk governs **which** packages enter, not npm's content addressing.

Issue #606 asks whether `requires` should move into each binding's own
`package.json`. That question stays open, but it is **not blocked** by this
design: the lock's `[[npm]]` rows have the same shape either way.

### 7.3 First-party npm publication

ADR 0310's emit-ABI package and `@bynk/wire-client` are the only Bynk-owned
npm packages in prospect. Neither is published today. This track owns their
publication mechanics: the npm org, provenance attestations, and a release
job beside `release.yml`. **`@bynk/wire-client` gets the ADR it never had**
when it is first published (§10).

## 8. Decisions

| # | Decision | Disposition |
|---|---|---|
| D1 | Shareable unit set: commons and adapters, behavioural units later, never contexts (spine Q2) | Kept from spine |
| D2 | Identity: `org/package` in the manifest; unit names unchanged; dependency units reached as `<alias>.<unit>`; workspace is not an identity tier (spine Q3) | **Re-settled.** Replaces the flat-name cutover (§3.3) |
| D3 | Backbone: one registry protocol, first-party and private instances, org→registry binding, git/path kept (spine Q1) | **Re-settled** (§6, §11 Q1) |
| D4 | Network boundary: only `bynk` fetches; compiler, LSP and wasm read a verified store via the in-memory source map | New (§6.4) |
| D5 | Integrity: sha256 over canonical archives and trees; FNV-1a stays for contract hashes | New (§5.2) |
| D6 | Single-version resolution; `version_conflict` is the honest failure (spine Q6) | Kept from spine |
| D7 | Lock schema (spine Q7) | Settled here (§5.2) |
| D8 | Mixin semantics across packages (spine Q4): a `uses` of a dependency's commons mixes in exactly as a local one does; opaque/transparent export and sealed construction are unchanged | Spec'd in slice 2 |
| D9 | First-party publish timing: runtime and bindings vendored forever; ambient and stdlib publish after the ABI freeze (spine Q5) | Kept from spine |
| D10 | `bynk.cloudflare` is one multi-unit package under reserved org `bynk` (spine Q10, Q11) | Kept, and consistent with P-3 because names are unchanged (§3.3) |
| D11 | Behavioural-unit declaration site (spine Q9) | Scoped out to slice 6's own ADR |
| D12 | Origin-signed index (ed25519, keys pinned in the binary or in `[registries]`, or locked on first use); signatures bind registry `id` and package `name`; the lock's `source` is authoritative against routing; git invocation is hardened, with `file` URLs confined like path dependencies; path dependencies are confined to the root and workspace members; keys rotate through a signed chain (planned rotation only) | New, from settling review (§6.2, §6.3, §11 Q10) |

## 9. Candidate slice decomposition

The scope is provisional until each slice's increment proposal lands. Each slice becomes an increment proposal,
filed as a sub-issue of #843.

| Slice | Content | Notes |
|---|---|---|
| ~~0~~ | ~~Flat-name cutover~~ | **Dropped under D2.** It returns, with the §3.3 ledger migration, only if D2 is revisited |
| 1 | Manifest and workspace surface: `[package]`, `[dependencies]`, `[dev-dependencies]`, `[registries]`, `[workspace]` parsed and validated; no resolution | **Done** (#2207). The lock skeleton moved to slice 2 (#2207 decision B): until something is resolved, a lock has nothing to record |
| 2a | Path dependencies end to end: the package graph; package-scoped name resolution (`<alias>.<unit>`, canonical names `$<org>_<package>.<unit>`); exports enforced; only reached units compiled; `deps/` emit layout; path-dependency confinement; D8 spec'd | **Done** (#2223). Slice 2 split into 2a and 2b (#2223 decision A) |
| 2b | Git sources; `bynk.lock` v1 with sha256 (the lock skeleton moved here from slice 1); the store; the `--locked` and `--offline` modes; hardened git invocation; one lock per workspace; `[dev-dependencies]` | **Done** (#2240). Range syntax moves to slice 4, where ranges are first interpreted; `source_mismatch` (a registry concern) and `bynk vendor` move to slice 4 too |
| 3 | npm propagation across packages; `conflicting_range` diagnostic; `required-by` | **Done** (#2255). The conflict fix shipped on its own first (#2201). Range intersection and a consumer override table are follow-ons |
| 4a | Registry protocol spec and client: index, signed lines bound to `id` and `name`, version selection, trust (pinned, locked on first use, rotation chain), routing, mirrors, `source_mismatch`, token auth, `bynk add`; the reference static registry (`bynk registry init`/`add`/`yank`); `file://` indexes | **Done** (#2274). Archives are tree-checksum transport (§6.1 amended). The lock records the trust *anchor*, so a planned rotation changes no lock. `bynk vendor` is dropped in favour of caching `$BYNK_HOME` |
| 4c | The HTTP publish API (`PUT <api>/<org>/<package>/<version>`, yank and unyank) and `bynk publish`/`bynk yank`; the reference server `bynk registry serve` with hashed, scoped tokens (`bynk registry token`) | **Done** (#2287). The registry signs uploads; the client verifies by read-back |
| 4b | The hosted first-party registry: service, org claiming, account policy, abuse handling | **Its own track:** [`first-party-registry.md`](first-party-registry.md) (#2308), a Bynk service in this repo, extracted after go-live |
| 5 | Adapter package trust: approve and diff, closing ADR 0020 | **Done** (#2296). Approval is per `(package, range)` in `bynk.lock`, enforced by the compile; `bynk approve` |
| 6 | Behavioural library units, including DO class naming (§3.4) | Gated on agent-capability-encapsulation; coordinate with #2188 |
| 7 | First-party `bynk.*` lift to the registry | Gated on the ABI freeze (≈1.0) |

## 10. Front-loaded ADRs

Numbers are taken at merge (ADR 0076).

1. **The shareable-unit set** (D1). Lands with slice 1.
2. **Package identity without renaming** (D2). This includes the alias rule,
   the reserved-root special case, and the workspace-is-not-identity
   departure. Lands with slice 1.
3. **The distribution backbone as a protocol** (D3), with the org→registry
   binding and the first-party publication rule. Lands with slice 2, so that
   git and path ride the same source model, and is completed in slice 4.
4. **The network boundary and integrity** (D4, D5, D12): the fetch-only driver, sha256, the identity-binding signed index, the locked TOFU fingerprint (`key_mismatch`), the planned-rotation-only caveat, `source_mismatch`, git hardening (including `file`-URL confinement) and path confinement. Lands with slice 2; the index signature is completed in slice 4.
5. **Single-version resolution and `bynk.lock`** (D6, D7). Lands with
   slice 2.
6. **Workspaces.** Lands with slice 1.
7. **Adapter package trust**, superseding ADR 0020. Lands with slice 5.
8. **Bynk-owned npm publication**, covering the ADR 0310 ABI package and
   `@bynk/wire-client`. It retroactively records the interop track's D3. Lands
   with the first publish.

## 11. Settling-review questions (all resolved)

Every question was closed with the maintainer on 2026-10-20, after the
settling review on #2199. Each one resolved to the draft's recommendation.

1. **Q1, the backbone. Resolved: a protocol, with each organisation bound to
   one registry (§6).** The registry is a protocol. Bynk runs one instance
   of it, and anyone can run a private one. Each organisation resolves on
   exactly one registry, through the root's `[registries]` map, and the
   first-party registry is the default. Two alternatives were rejected:
   - a registry per dependency (Cargo style), because it lets one
     `org/package` resolve from two places in a graph;
   - the spine's first-party-plus-git, because it gives private code no
     integrity or versioning story beyond a git rev.
2. **Q2, cross-package references. Resolved: `<alias>.<unit>` (§3.3).** The
   consumer's own units are never renamed. The first-party `bynk.*` surface
   keeps its reserved full names as the one special case. Two alternatives
   were rejected:
   - globally unique names, because any publisher could break a consumer by
     adding a unit;
   - the full cutover, because it orphans deployed state (§3.3) and must
     precede #2188 (§13).

   Slice 0 is dropped.
3. **Q3, workspaces. Resolved: a build grouping only (§3.2, §4.2).**
   Workspace is not an identity tier. That departs from the spine's four
   tiers.
4. **Q4, manifest tables. Resolved: a separate `[package]` (§4.1).**
   `[project]` stays for applications, and a project with both tables is
   refused.
5. **Q5, bare-string dependencies. Resolved: not allowed.** Every dependency
   is a table that names its `package`. No organisation is ever inferred.
6. **Q6, the store. Resolved: a per-user cache (§6.4).** It lives at
   `~/.cache/bynk/store/sha256/<hex>`, is immutable, is verified on insert,
   and is shared across projects. `bynk vendor` covers CI with no network.
7. **Q7, git. Resolved: shell out to `git`, hardened (§6.3).** Adding a
   pure-Rust git crate was rejected as too heavy a dependency for a
   bootstrap path.
8. **Q8, semver honesty. Resolved: deferred.** Publish does not check it in
   slice 4. A later warning needs a "compatible" relation over exported
   signatures, which the ADR 0200 contract hash (it only says "changed")
   does not provide. This is recorded as a follow-on.
9. **Q9, the first-party service's implementation. Resolved: deferred to
   slice 4b's own track.** This track fixes only the protocol the service
   must speak.
10. **Q10, signed-index depth. Resolved: signed lines plus key rotation
    (§6.2, D12).** Each line carries an ed25519 origin signature. Keys are
    pinned in the binary (first-party), in `[registries]` (private), or in
    the lock for a TOFU key. Each signature binds the registry `id` and the
    package `name`. A signed key-rotation chain in `config.json` covers
    **planned** rotation without a release or a manifest edit. Recovering
    from a compromised key needs a toolchain release (first-party) or a
    `[registries]` key edit (private), because a leaked old key can sign
    its own chain. Full TUF is deferred: revocation, threshold roles, and
    freshness against freeze attacks on first resolution. For locked
    versions, the lock already defeats rollback. The re-review on #2199
    added the identity binding, the locked TOFU fingerprint and the
    compromise caveat.

## 12. Threat model

The track distributes code that the consumer **compiles and ships**:

- **Resolving Bynk source executes nothing.** There are no install scripts and
  no build step. The only executable risk is the npm sub-layer, which slice 5
  fences.
- **Integrity.** Archives are content-addressed (sha256), pinned in the lock,
  verified on fetch and on store insert, and immutable. Yanking never
  deletes.
- **Dependency confusion is structural (§6.2).** An org resolves on exactly
  one registry, from the root's map only, and the first-party registry
  refuses externally resolved dependencies. The residual case, a root map
  missing an entry, is closed by the `source_mismatch` rule and by
  claim-blocking (below).
- **Mirrors and caches cannot substitute content.** They can only withhold
  it. For a locked version this follows from the hash pin. For first
  resolution it follows from the origin-signed index (D12). That index
  binds the registry `id` and the package `name`, so a signed line cannot
  be replayed under another name. The guarantee holds only as far as the
  origin's key is trusted:
  - a private registry with no pinned key is trust on *first* use, with the
    fingerprint locked after that;
  - a compromised key is not recovered by rotation. It needs a release or a
    manifest edit (§6.2).
- **A misrouted organisation fails, not falls through.** A lock `source`
  that disagrees with the current routing is a hard error. Claim-blocking on
  the first-party registry covers organisations that are not yet locked.
- **Fetching never runs code.** The git invocation is hardened (§6.3), and
  path dependencies from fetched packages are refused, so no fetched tree
  can reach outside itself.
- **Credentials** never appear in the manifest, the lock or the archives. They
  are sent only to their registry's origin and dropped on a cross-origin
  redirect.
- **Namespace safety.** The reserved org `bynk` cannot be rebound or
  published to by a third party, and the reserved unit root `bynk` cannot be
  an alias.
- **The type-boundary trust model is not widened.** Opaque and transparent
  export and sealed construction hold across a package boundary exactly as
  across a context boundary. A dependency cannot construct a consumer's
  sealed types, nor reach beyond its declared `exports`.
- **Platform lock is part of the contract.** A Cloudflare-locked unit's lock
  (ADR 0024) surfaces at resolve time, not at deploy.
- **Deployed state is not renamed by this track (§3.3).** The orphaning risk
  comes back only if D2 is revisited, and slice 6 is the one place
  qualified names reach deployed state (§3.4).

## 13. Sequencing

`bynk-adoption-sequencing.md` orders the adoption blockers deploy →
migrations → ecosystem. Deploy is shipped. State migrations (#2188) is next.
Packaging is adoption-critical but 1.0-optional.

**Under D2 there is no hard ordering dependency on #2188.** Packaging renames
nothing that migrations keys on: agent names, class names and ledger keys are
untouched. Slices 1 and 2 touch `bynk-project`, `bynk-check` and the driver's
discovery, and can interleave with migrations work without conflict.

**Under the spine's cutover there is a hard dependency.** The rename must
land *before* migrations begins recording stored state shapes keyed by
agent name, or every user takes two migrations: one for the rename and one
for the shape. This asymmetry is the strongest practical argument for D2.

Slice 6 must coordinate with #2188 whichever way D2 settles (§3.4). Slice 7
waits on the ABI freeze.

## 14. Non-goals

- **Sharing contexts.** A context is a deployment unit, not a library.
- **Multiple versions of one package in a graph.**
- **Running code at resolve time.** Bynk packages have no install scripts and
  no build step.
- **Replacing npm for the adapter layer.**
- **Publishing the codegen.** Only the ADR 0310 ABI and the wire client
  cross the npm boundary.
- **Renaming the binding-level `requires`.** Issue #606 owns that question.
