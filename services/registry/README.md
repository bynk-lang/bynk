# The first-party Bynk registry

The hosted instance of the [registry protocol](../../site/src/content/docs/docs/registry-protocol.md):
the registry every organisation not routed elsewhere resolves on. It is
written in Bynk and built with this repository's own toolchain. The design is
the first-party registry track, [`design/tracks/first-party-registry.md`](../../design/tracks/first-party-registry.md)
(spine [#2308](https://github.com/bynk-lang/bynk/issues/2308)).

It lives here until the protocol and the `bynk-std` surface it uses settle,
then moves to its own repository (track §1.3). Until then it keeps the
extraction rules: it reaches only `bynk-std`, its own files and lock-pinned
git dependencies — never a path into the compiler, never a registry
dependency — and no account-specific configuration is committed.

## Layout

| Path | What |
|---|---|
| `src/registry/store.bynk` | `context registry.store`: owns the registry's bucket (`capability Store = R2`) and serves the protocol's read paths |
| `src/registry/paths.bynk` | `commons registry.paths`: the object keys, and the name rules that guard them |
| `tests/` | Unit tests: the key rules, and the read paths against a stubbed bucket |
| `seed/seed.sh` | Upload a `bynk registry` directory into the bucket |
| `scripts/check-read-path.sh` | The read path end to end, as CI runs it |

**One context owns the bucket.** Under the standard library's resource model a
bucket belongs to the context that declares it, so the read service here, and
the publish service and per-package index writers that join it later, all live
in `registry.store`.

**The bucket mirrors a static registry.** Its keys are exactly a
`bynk registry` directory's paths: `config.json`, `<org>/<package>` (index
files) and `dl/sha256/<hex>` (archives). A seed is a copy of such a directory,
and an export is the reverse.

## Reading

| Request | Serves | Content type |
|---|---|---|
| `GET /config.json` | `config.json` | `application/json` |
| `GET /<org>/<package>` | the package's index file | `application/x-ndjson` |
| `GET /dl/sha256/<hex>` | an archive | `application/vnd.bynk.tree-v1` |

A missing object, or a name outside the manifest's rules (a malformed
organisation, package or digest), is `404`; a malformed name never reaches the
bucket. Nothing here signs or decides anything: clients verify every line's
signature and every archive's checksum. The read paths aren't cached yet —
that is the language slice L3 ([#2335](https://github.com/bynk-lang/bynk/issues/2335)).

## Seeding

There is no publish API yet (R2), so the bucket is seeded from a registry
built offline with the `bynk registry` tooling. The root key stays offline; a
delegate signs:

```bash
bynk registry init ./reg --id bynk --key-out ~/secrets/root.key
bynk registry delegate ./reg --key ~/secrets/root.key --delegate-out ~/secrets/online.key
bynk registry add ./reg ./packages/hello --key ~/secrets/online.key
seed/seed.sh ./reg --bucket <bucket-name>
```

`seed.sh` copies every file (skipping dot-directories) to an object keyed by
its relative path. It carries no keys: the directory arrives signed.
`WRANGLER` may name the command to run (`npx -y wrangler@4`).

## Running it locally

```bash
BYNK=$(command -v bynk) scripts/check-read-path.sh
```

builds a fixture registry (`conformance-fixture/hello`, two versions, one
yanked), seeds `bynk dev`'s local bucket, serves the service, and runs
`bynk registry conformance --read-only` against it. To explore by hand, run
`bynk dev . -- --persist-to <dir>` and seed with
`seed/seed.sh <reg> --bucket registry-store-store --local --persist-to <dir>`.

R2 is experimental in the standard library, so `bynk.toml` opts in under
`[unstable] allow`; this service is the workload it graduates on.
