# 0563 — The registry protocol — config, signed index lines, tree-checksum archives

- **Status:** Accepted (v0.383)

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
