---
level: patch
changelog: The registry's admission rules move into the I/O-free `bynk-registry-core` crate, which `bynk registry add` and `serve` link and which also builds to a standalone wasm32 module
---

## ADR: registry-admission-is-one-io-free-crate
title: The registry's admission rules are one I/O-free crate, native and wasm
summary: bynk-registry-core holds unpack, pack and admit with no filesystem, ring or network; it links natively and builds to a C-ABI wasm32 module, proved by a size budget and a native-vs-wasm parity job

**Context.** Admission is the registry's most security-sensitive code: the
hardened archive reader (traversal, ordering, size caps), the canonical
checksum, and the manifest, name, version and dependency rules. The hosted
registry (R2) must apply exactly the rules the client and the reference server
do, and reimplementing them in Bynk would need octet-level `Bytes`, a TOML
parser and a second implementation free to drift (track §3.4, D3). The code
lived in the `bynk` binary crate, where it staged an upload through a
temporary directory (with `ring` randomness) and read it back with `std::fs`,
so it could not be compiled for a Worker.

**Decision.**
- A new leaf crate, `bynk-registry-core`, depends on `bynk-project`, `sha2`,
  `toml` and `serde_json` only: no `std::fs`, no `ring`, no network. It
  provides `unpack(bytes) -> Tree` (an in-memory, ordered path-to-bytes map,
  applying the client's hardening rules unchanged), `pack(&Tree) ->
  (bytes, checksum)` and `admit(upload, expected) -> Result<Admitted,
  Refusal>`. `admit` checks the manifest, the expected name and version and
  registry-only dependencies, collects `npm` requirements from the in-memory
  tree's adapter sources, and returns the canonical archive, its checksum, the
  dependencies and the npm requirements. Signing, the index and every write
  stay with the caller.
- `bynk`'s `archive` and `commands::admit` call the crate. The staging
  directory and `private_staging` are gone, and `npm_of` reads the tree rather
  than the disk. The archive format is unchanged (it never used tar or
  compression, so the module carries no compression backend).
- The wasm build is a plain C-ABI `cdylib` with no `wasm-bindgen`: `alloc` and
  `dealloc`, and `admit(ptr, len, expected_ptr, expected_len) -> u64`, a packed
  pointer and length to a UTF-8 JSON result in linear memory. The module has no
  imports and needs no glue, so it is the single file a binding's
  `includes` copies. `scripts/build-registry-core.sh` builds it and runs
  `wasm-opt` when present.
- Proof: CI builds the module, enforces a 1.5 MiB budget after `wasm-opt` (a
  breach trims a dependency, never the budget), and runs the admission corpus
  (good, mismatched name and version, junk, traversing, path-dependency, a
  registry dependency, an npm requirement, hidden and built files) through the
  native crate and through the `.wasm` on Node, requiring byte-identical
  results. `release.yml` attaches the module to each release so the service can
  pin a released copy.

**Consequences.** One implementation of the admission rules serves the client,
the reference server and, through an adapter, the hosted service. A rule change
is one edit in one crate, and the parity job catches native and wasm diverging.
`bynk-registry-core` is a published crate (a dependency of `bynk`), so it joins
the release publish order after `bynk-project`. The module's JSON result carries
the canonical archive as base64, which the service decodes; that is the price of
a one-function boundary with no glue. Loading the module behind an adapter
binding is what `includes` (#2326) provides.
