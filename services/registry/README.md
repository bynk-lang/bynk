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
| `src/registry/store/files.bynk` | `context registry.store`: owns the registry's bucket (`capability Store = R2`) and serves the protocol's read paths |
| `src/registry/store/publish.bynk` | The publish API: the `api` service, the per-package `PackageIndex` agent, operator tokens and delegate signing |
| `src/registry/admission.bynk` | `adapter registry.admission`: the shared rules (`bynk-registry-core`) as a wasm module |
| `scripts/admission-module.sh` | Build that module into `src/registry/` (git-ignored) |
| `src/registry/paths.bynk` | `commons registry.paths`: the object keys, and the name rules that guard them |
| `tests/` | Unit tests: the key rules, and the read paths against a stubbed bucket |
| `seed/seed.sh` | Upload a `bynk registry` directory into the bucket |
| `scripts/check-read-path.sh` | The read path end to end, as CI runs it |
| `scripts/check-publish.sh` | The publish API end to end, as CI runs it |

**One context owns the bucket.** Under the standard library's resource model a
bucket belongs to the context that declares it, so the read service, the
publish service and the per-package index writers all live in `registry.store`.

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

## Publishing

The publish API is the protocol's ("Publish API"), answered as
`bynk registry serve` answers it, so `bynk publish` and `bynk yank` work
against it unchanged:

| Request | Does | Answers |
|---|---|---|
| `PUT /api/<org>/<package>/<version>` | admit the archive, store it, sign and append its line | `201` and the line; `200` and the existing line for the same tree again |
| `POST /api/<org>/<package>/<version>/yank` | mark the version yanked | `200` `{"ok": true}` |
| `POST /api/<org>/<package>/<version>/unyank` | clear it | `200` `{"ok": true}` |

Every refusal is JSON `{"error", "message"}` with the reference server's status:
`400 invalid_package` for an archive the rules refuse (or one naming another
package or version), `401 unauthorised`, `403 forbidden`, `404 not_found`,
`409 version_exists` for another tree under a published version, and
`413 too_large` past the 64 MiB cap (the router's own refusals are rendered by
`on rejected`).

**The rules are the shared crate's.** Admission, the bytes a line's signature
covers, and what `config.json` says about a key all come from
`bynk-registry-core`, the code the client and the reference server use, run as a
wasm module through `adapter registry.admission`. The module is built, not
committed: run `scripts/admission-module.sh` once after a clone, and again after
a change to the rules (it needs the `wasm32-unknown-unknown` Rust target).

**One writer per package.** `PackageIndex[<org>/<package>]` holds the package's
lines and decides each append: new, a retry of the same tree, or a conflict. The
index file in the bucket is its projection, rewritten after each change. A
package that was seeded before the agent existed is adopted from its index file
on the first call.

### Secrets

| Secret | What |
|---|---|
| `REGISTRY_SIGNING_KEY` | The online delegate's private key: the PKCS#8 file `bynk registry delegate --delegate-out` writes, base64-encoded |
| `REGISTRY_SIGNING_PUBLIC` | Its public half, `ed25519:…` (what `delegate` prints) |
| `REGISTRY_TOKENS` | The publish tokens, as JSON: `[{"sha256", "organisations", "can"}]` |

**The root key never reaches the service.** Before every signature the service
checks its key against the bucket's `config.json`: a delegation from the newest
chain key names it and verifies, and no revocation names it; and a signature it
makes verifies. A key that fails is never used, and the publish is refused
(`500 internal`, naming the misconfiguration).

**Tokens are operator-issued until accounts (R3).** Mint each with
`bynk registry token <file> --org <org> [--can publish,yank]`, which prints the
token once and writes only its sha256; convert the file to the secret's JSON
(`python3 -c 'import json,sys,tomllib; print(json.dumps(tomllib.load(open(sys.argv[1],"rb"))["token"]))' tokens.toml`).
Changing a token is updating the secret.

## Seeding

The bucket starts as a registry built offline with the `bynk registry` tooling:
at least its `config.json`, with `"api": "api"` so clients find the publish API.
The root key stays offline; a delegate signs:

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
BYNK=$(command -v bynk) scripts/check-publish.sh
```

builds a fixture registry (`conformance-fixture/hello`, two versions, one
yanked), seeds `bynk dev`'s local bucket, serves the service, and runs
`bynk registry conformance --read-only` against it. To explore by hand, run
`bynk dev . -- --persist-to <dir>` and seed with
`seed/seed.sh <reg> --bucket registry-store-store --local --persist-to <dir>`.

`check-publish.sh` builds an empty registry with a delegate and an `api`,
serves it with two tokens, and runs the full `bynk registry conformance`; then
publishes, yanks and resolves a package with the real client; then restarts the
service with a key that isn't a delegate and checks it refuses to sign.
`BYNK_HOLD=1` leaves the service up after it starts, for exploring by hand.

R2 and `bynk.crypto` are experimental in the standard library, so `bynk.toml`
opts in under `[unstable] allow`; this service is the workload they graduate on.
