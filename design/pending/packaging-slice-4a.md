---
level: minor
probes: diagnostic_coverage
changelog: Packaging slice 4a — registries. A `{ package, version }` dependency resolves on the registry its organisation routes to in `[registries]`, at the highest version every semver range admits; `bynk` verifies each index line's ed25519 signature (bound to the registry `id` and the package name) against a pinned key or one trusted on first use and recorded in `bynk.lock`, follows key rotation, fetches the archive into the store only when its tree checksum is the signed one, and refuses a misrouted organisation (`bynk.registry.source_mismatch`). Tokens go only to the registry's own origin. `bynk add` writes a registry dependency; `bynk registry init`/`add`/`yank` build a static registry any file host can serve, or that a project reads as `file://`. The first-party registry isn't live yet
---

## ADR: the-registry-protocol
title: The registry protocol — config, signed index lines, tree-checksum archives
summary: Packaging slice 4a — `config.json` (protocol, id, dl, auth, key chain); append-only JSON index lines signed over a canonical payload binding `registry` and `name`; archives are `bynk-tree-v1` streams verified by the store's canonical-tree checksum

**Context.** Track §6.1 sketched a small protocol a static host can serve, with archives as byte-canonical tars
addressed by their own hash. Slice 2b (#2240) already names store entries by a canonical-tree checksum.

**Decision.**

- `config.json` holds `protocol` (1), `id` (bound by every signature), `dl` (the archive base), `auth`
  (`none` or `token`) and `keys`, the rotation chain: each key after the first carries the previous key's
  signature over `{"key","registry"}`.
- `<org>/<package>` holds one JSON line per version — `name`, `version`, `checksum`, `deps`, `npm`, `toolchain`,
  `yanked`, `sig` — append-only; only `yanked` may change. `sig` is ed25519 over the canonical payload: every
  field but `sig` and `yanked`, plus `registry`, as JSON with byte-wise ordered keys and no insignificant
  whitespace. A client accepts a line only when a key the trusted chain reaches signed it, its `registry` is
  the trusted id and its `name` is the package asked for (`bynk.registry.bad_signature`).
- **Archives are transport only** (amending §6.1): the signed checksum is the canonical-tree sha256, and an
  archive is `bynk-tree-v1\n` followed by the files as `path \0 length \0 bytes` — the very stream that checksum
  hashes. Unpacking takes plain relative paths only, in order, unrepeated, within size limits
  (`bynk.registry.bad_archive`), and the tree is admitted only when its checksum is the signed one. No tar, so no
  links, modes or traversal; registry and git packages share one store and one verification.
- ed25519 comes from `ring`, already in the build through rustls; `semver` is the one new crate. Build output,
  `node_modules/` and hidden files are never packed.

**Consequences.** A static directory is a complete registry. The HTTP publish API (slice 4c) and the hosted
first-party service (4b) build on the same files.

## ADR: registry-trust-and-routing
title: A registry's key is pinned or trusted on first use, the lock records the trust anchor, and an organisation resolves in one place
summary: Trust order — `[registries] key`, else `bynk.lock`'s `[[registry]]` fingerprint, else first use (announced and recorded); every key the rotation chain reaches from the anchor is accepted; a changed `id` or an unreachable key is `key_mismatch`; routing by organisation, mirrors verified against the origin, `source_mismatch`, and the first-party fallback `unavailable` until 4b

**Context.** Track §6.2 and D12 settled trust on first use with a locked fingerprint, rotation by chain, and
organisation routing that makes dependency confusion structural.

**Decision.**

- A client trusts a key pinned in `[registries.<name>] key`; else the fingerprint `bynk.lock` recorded; else the
  newest key in `config.json` on first use, which it announces and records. It accepts every key the rotation
  chain reaches **from** that trusted key. The lock records that trust **anchor**, not the newest key, so a
  planned rotation changes no lock and lines signed before it still verify. A config whose keys don't lead
  from the anchor, or that declares a different `id` than the lock recorded, is `bynk.registry.key_mismatch`.
  Recovering from a compromised key is a manual pin and `bynk update`.
- An organisation routes to the registry whose `organisations` lists it — the root's (or workspace's) map
  only, with no per-dependency override — else to the first-party registry, which isn't live yet
  (`bynk.registry.unavailable`; one constant enables it in 4b). A locked package whose organisation now routes
  elsewhere is `bynk.registry.source_mismatch`, in every mode.
- A mirror is a copy of the origin's files (its `config.json` included); lines read through it are verified
  against the origin's trust.
- A token comes from `BYNK_REGISTRY_<NAME>_TOKEN` or `~/.config/bynk/credentials.toml` and is sent only to the
  exact origins of the registry's index and archive base; redirects are followed by hand, a cross-origin one
  drops it, and `https`→`http` is refused (`bynk.registry.http_failed`). It never appears in a file or a message.

**Consequences.** A locked build with its trees in the store reads no registry at all; a rotation is followed
the next time an index is read.

## ADR: registry-version-selection
title: A registry package resolves to the highest version every range admits, without backtracking
summary: Cargo-style semver ranges (validated in the manifest); one version per package across the workspace; the lock wins while it satisfies every range, even when yanked; yanked and too-new-toolchain versions are never newly chosen; resolution repeats to a fixpoint and reports an unsatisfiable set as `version_conflict` naming each requirer

**Context.** Range syntax validation was moved to this slice (#2223, #2240); track D6 is single-version
resolution.

**Decision.** A `version` is a Cargo-style semver range (`^1.4`, `~1.4.2`, `=1.4.2`, comparator lists; a bare
`1.4` is `^1.4`), validated as the manifest is read. Each registry package resolves to the highest non-yanked
version satisfying every range reaching it — from the project, workspace members and other packages — whose
`toolchain` this `bynk` meets; a chosen version's own dependencies add ranges, and resolution repeats until
none appears. The locked version wins while it satisfies every range, even when yanked; `bynk update`
re-chooses. No version satisfying every range is `bynk.package.version_conflict`, naming each requirer and
range. There is no backtracking search.

**Consequences.** An unsolvable-looking graph that a backtracking resolver could solve is reported, not solved;
that is a recorded follow-on if real graphs need it.
