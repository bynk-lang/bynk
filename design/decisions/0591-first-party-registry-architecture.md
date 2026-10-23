# 0591 — The first-party registry is a Bynk service in this repo, one context owning one bucket in the static layout, held to the protocol by conformance

- **Status:** Accepted (v0.393.2)

**Context.** The packaging track deferred the hosted first-party registry to its own track (#2308), which settled
the service's architecture in §1.3, §3 and §7 (decisions D1, D2, D3, D6) and scheduled this ADR to land with
the first service slice. R1 (#2334) is that slice. Building it surfaced one correction: a bucket belongs to the
context that declares it (standard-library D3), so the track's first sketch of separate `read` and `publish`
contexts couldn't own one bucket between them.

**Decision.**

- **Written in Bynk, here, extracted later (D1).** The service is a Bynk project at `services/registry/`, built
  with this repository's toolchain, under extraction rules E1–E5: it reaches only `bynk-std`, its own files and
  lock-pinned git dependencies; it has no registry dependencies; no account-specific configuration is
  committed; it has its own CI job. It moves to its own repository after go-live plus two `bynk-std` releases
  that needed no change for it.
- **One context owns one bucket, in the static layout (R1).** `context registry.store` declares
  `capability Store = R2`; the read service, the publish service and the per-package index writers live in it,
  and other contexts reach its data through its services. The bucket's keys are exactly a `bynk registry`
  directory's paths — `config.json`, `<org>/<package>`, `dl/sha256/<hex>` — so seeding is a directory copy, an
  export is the reverse, and the protocol's paths map one-to-one onto keys.
- **A single writer per package (D2).** A `PackageIndex` agent keyed by `org/package` serialises a package's
  appends and yanks; its committed state is authoritative and the index file in the bucket is its projection
  (D10).
- **Shared admission (D3).** Uploads are admitted by `bynk-registry-core`, the same Rust rules the client and
  the reference server use, compiled to a C-ABI wasm module behind an adapter.
- **Conformance defines the protocol (D6).** The service is held to `bynk registry conformance` — read-only in
  CI from R1, the full run from R2 — exactly as `bynk registry serve` is.
- **Seeding before publishing (R1).** Until the publish API exists, the bucket is seeded from a registry built
  and signed offline with the `bynk registry` tooling (an offline root and a delegate); the seed carries no
  keys.

**Consequences.** The service is a real, verifiable read-only registry before any write path exists, and every
later slice adds to one context and one bucket. Its read paths serve `Raw` responses, which carry no HTTP
validator and can't be `@cache`d; caching them is the language slice L3, which go-live waits on.
