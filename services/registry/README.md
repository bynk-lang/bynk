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
| `src/registry/store/publish.bynk` | The publish API: the `api` service, the per-package `PackageIndex` agent, the token check and delegate signing |
| `src/registry/store/trusted.bynk` | Trusted publishing: the `Workload` actor (GitHub Actions OIDC), trusted publishers, and the exchange |
| `src/registry/store/accounts.bynk` | Accounts: the `account` service, GitHub's device flow, and the `Organisation`, `PublishToken` and `Session` agents |
| `src/registry/admission.bynk` | `adapter registry.admission`: the shared rules (`bynk-registry-core`) as a wasm module |
| `scripts/admission-module.sh` | Build that module into `src/registry/` (git-ignored) |
| `src/registry/paths.bynk` | `commons registry.paths`: the object keys, and the name rules that guard them |
| `tests/` | Unit tests: the key rules, and the read paths against a stubbed bucket |
| `seed/seed.sh` | Upload a `bynk registry` directory into the bucket |
| `scripts/check-read-path.sh` | The read path end to end, as CI runs it |
| `scripts/check-publish.sh` | The publish API end to end, as CI runs it |
| `scripts/check-accounts.sh` | Accounts end to end, against `scripts/fake-github.py`, as CI runs it |
| `scripts/check-trusted.sh` | Trusted publishing end to end, against `scripts/fake-oidc.mjs`, as CI runs it |
| `scripts/check-limits.sh` | The rate limits and the quota end to end, as CI runs it |
| `scripts/check-export.sh` | The export and a restore drill end to end, as CI runs it |

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
`409 version_exists` for another tree under a published version, `410 withdrawn`
for a version taken down (and `409 withdrawn` to un-yank one), and
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
index file in the bucket is its projection, rewritten by the agent inside the
call that made the change, so concurrent changes never land out of order. A
package that was seeded before the agent existed is adopted from its index file
on the first call; a line there that doesn't parse is refused, never dropped.

### Secrets

| Secret | What |
|---|---|
| `REGISTRY_SIGNING_KEY` | The online delegate's private key: the PKCS#8 file `bynk registry delegate --delegate-out` writes, base64-encoded |
| `REGISTRY_SIGNING_PUBLIC` | Its public half, `ed25519:…` (what `delegate` prints) |
| `REGISTRY_TOKENS` | The operator tokens, as JSON: `[{"sha256", "organisations", "can"}]` (optional) |
| `GITHUB_CLIENT_ID` | The registry's GitHub OAuth app (see "Accounts") |
| `GITHUB_BASE`, `GITHUB_API_BASE` | GitHub's sign-in and API origins; default `https://github.com` and `https://api.github.com` (tests point them at a fake) |
| `REGISTRY_OIDC_TRUST` | Optional, never in production: another OIDC issuer for the `Workload` actor (see "Trusted publishing") |
| `GITHUB_API_TOKEN` | Optional: a GitHub token for looking members up, to lift GitHub's anonymous rate limit |
| `REGISTRY_QUOTAS` | Optional: raised storage quotas, as JSON `[{"org", "bytes"}]`; an organisation not listed has 1 GiB (see "Limits") |

**The root key never reaches the service.** Before every signature the service
checks its key against the bucket's `config.json`: a delegation from the newest
chain key names it and verifies, and no revocation names it; and a signature it
makes verifies. A key that fails is never used, and the publish is refused
(`500 internal`, naming the misconfiguration).

**Operator tokens** are the bootstrap and CI path, beside the tokens people
mint through accounts. They don't expire, and an operator manages them by
updating the secret. A token whose `can` includes `operate` may also use the
account API as any organisation's owner: that is how a reserved name is claimed
for its rightful owner, and how a dispute is settled. Mint each with
`bynk registry token <file> --org <org> [--can publish,yank]`, which prints the
token once and writes only its sha256; convert the file to the secret's JSON
(`python3 -c 'import json,sys,tomllib; print(json.dumps(tomllib.load(open(sys.argv[1],"rb"))["token"]))' tokens.toml`).
Changing a token is updating the secret.

## Accounts

People sign in with GitHub (the OAuth **device flow**: no password, no
cookie, no GitHub credential kept), claim organisations, add members, and mint
the publish tokens `bynk publish` uses. The `bynk` client drives it (R3.1,
#2378; see the CLI reference for `bynk login` and `bynk org`):

```bash
bynk login --registry <name>                # prints a code to approve at GitHub, then waits
bynk org claim acme                         # first come; --block holds the name
bynk org member add acme <github-login>
bynk org token mint acme --days 90 --save   # into acme's slot in the credentials file, for bynk publish
bynk org show acme
```

The API underneath is JSON, so `curl` drives it too:

```bash
# Sign in: open verificationUri, enter userCode, then poll every `interval` seconds.
curl -X POST https://<registry>/account/login
curl -X POST https://<registry>/account/login/poll -H 'content-type: application/json' -d '{"poll":"<poll>"}'
#   400 authorization_pending — not yet; 429 slow_down — poll less often; 200 — {"token": "bynka_…"}

# Claim an organisation (first come), add a member, mint a publish token.
curl -X POST https://<registry>/account/orgs/acme -H "authorization: Bearer $ACCOUNT" -H 'content-type: application/json' -d '{"block":false}'
curl -X PUT  https://<registry>/account/orgs/acme/members/<github-login> -H "authorization: Bearer $ACCOUNT"
curl -X POST https://<registry>/account/orgs/acme/tokens -H "authorization: Bearer $ACCOUNT" -H 'content-type: application/json' \
  -d '{"can":["publish","yank"],"days":90,"note":"laptop"}'
#   201 — {"token": "bynk_…" (shown once), "info": {"id": …, "expiresAt": …}}
```

| Request | Who | Does |
|---|---|---|
| `POST /account/login`, `POST /account/login/poll` | anyone | sign in; the account token lasts 30 days |
| `DELETE /account/session` | the session | sign out |
| `POST /account/orgs/<org>` `{"block", "owner"?}` | a person; an operator for someone (`owner`) | claim a free name; `block: true` holds it without publishing |
| `GET /account/orgs/<org>` | members | the owner, members, whether it is blocked, and its tokens (never the tokens themselves) |
| `PUT /account/orgs/<org>/blocked` `{"blocked"}` | the owner | block or unblock |
| `PUT /account/orgs/<org>/members/<login>` | the owner | add a member (stored by GitHub's numeric id) |
| `DELETE /account/orgs/<org>/members/<id>` | the owner | remove a member (the owner stays) |
| `POST /account/orgs/<org>/tokens` `{"can", "days", "note"}` | members | mint a publish token for 1 to 365 days |
| `DELETE /account/orgs/<org>/tokens/<id>` | the owner; the member who minted it | revoke it |

**Names.** An organisation follows the manifest rule. A short list is reserved
(`bynk` and `bynk-*`, `cloudflare`, `workers`, `npm`, `node`, `deno` and
impersonations of them; `registry.paths.isReserved`); ask the operators for
one. A **blocked** organisation refuses every publish. Block a name before
anything is seeded into it: a block refuses publishes, and doesn't hide an
index file already in the bucket.

**Two kinds of token, never interchangeable.** An account token (`bynka_…`)
manages organisations and can't publish; a publish token (`bynk_…`) publishes
and/or yanks for one organisation, and can't manage anything. Both are stored
only as their sha256, and both expire. A publish token works only while its
organisation lists it: removing a member ends every token they minted, and
adding them back doesn't revive those tokens.

**Never enter a code you didn't request.** The device flow's page is GitHub's
own and names the app; someone who persuades you to approve their code signs in
as you.

**The GitHub OAuth app.** Create one under the organisation that runs the
registry, enable **device flow**, and leave the callback URL empty. The device
flow needs no client secret, so none is created; set its client id as
`GITHUB_CLIENT_ID`.

## Trusted publishing

A GitHub Actions job can publish with **no stored secret**. It asks GitHub for
an OIDC token for this registry, exchanges it here for a publish token that
lasts 15 minutes, and runs `bynk publish`. An organisation's owner says which
repository, which workflow file and, optionally, which GitHub environment may:

```bash
curl -X POST https://<registry>/account/orgs/acme/trusted -H "authorization: Bearer $ACCOUNT" \
  -H 'content-type: application/json' \
  -d '{"repository":"acme/money","workflow":".github/workflows/release.yml","environment":"release","can":["publish"]}'
```

The repository is stored by **GitHub's numeric id**, which is what an exchange
matches, so a repository renamed, deleted and recreated, or transferred, doesn't
inherit the trust. `DELETE /account/orgs/<org>/trusted/<id>` removes one (the
publish tokens it already minted run out within 15 minutes); the organisation's
view lists them. When several publishers match a job, one bound to its
environment is used before one that takes any. Each OIDC token is exchanged
once: its `jti` is remembered for a day.

The workflow side:

```yaml
jobs:
  release:
    environment: release            # recommended: a protected environment
    permissions:
      id-token: write
      contents: read
    steps:
      - uses: actions/checkout@v4
      - name: Publish
        run: |
          oidc=$(curl -sS -H "Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
            "$ACTIONS_ID_TOKEN_REQUEST_URL&audience=bynk-registry" | jq -r .value)
          token=$(curl -sS -X POST https://<registry>/account/trusted/exchange \
            -H "authorization: Bearer $oidc" -H 'content-type: application/json' \
            -d '{"org":"acme"}' | jq -r .token)
          BYNK_REGISTRY_<NAME>_TOKEN="$token" bynk publish
```

An exchange is refused (`403 not_trusted`) unless a trusted publisher of the
organisation matches the token's `repository_id`, the workflow file in its
`job_workflow_ref`, and its `environment` (when one is registered); a token for
another audience, or an expired one, is `401`; a blocked organisation is `403
blocked`. **Use a protected environment** (required reviewers, restricted
branches): it is what keeps an arbitrary branch of the repository from
publishing through the release workflow.

`REGISTRY_OIDC_TRUST` (optional) points the `Workload` actor at another issuer —
JSON `{"issuer", "jwks"}` — for tests and staging. **Leave it unset in
production**, where GitHub's own issuer applies.

## Limits

Cloudflare's rate limiting (`src/registry/store/limits.bynk`) counts, per
Cloudflare location and approximately:

| Limit | Per | Applies to |
|---|---|---|
| 60 a minute | publish token (by its sha256) | `PUT`, yank and unyank |
| 600 a minute | IP (`CF-Connecting-IP`) | the same: a flood guard only, since GitHub-hosted runners share addresses |
| 60 a minute | IP | the account API: sign-in, polling, organisations, tokens |
| 10 a minute | repository (the token's `repository_id`) | the trusted-publishing exchange |

Past one, a request is `429 rate_limited`. Reads aren't limited by the service:
they're cached at the edge, and read abuse is a matter for Cloudflare's WAF.
The limits are in the source, so changing one is a release; `bynk deploy`
allocates each limiter's namespace id into `bynk.deploy.lock`.

Each organisation has a **storage quota**: the bytes of its uploads, 1 GiB
unless `REGISTRY_QUOTAS` raises it. An upload past it is `413 quota_exceeded`,
naming the usage. A retry of a tree already published passes whatever the
usage. The count is kept in an `OrgUsage` agent, checked before an archive is
stored and added after the line commits, so concurrent publishes to different
packages can overshoot it by one upload each.

## Export and restore

An hourly cron (`17 * * * *`, `src/registry/store/export.bynk`) copies
everything to a second bucket, the `Backup` capability's. A pass starts a day
after the last one began, and each tick does one step: a page of the store
bucket (up to 1000 objects), or a batch of 200 organisations. A pass copies:

- every object in the store bucket: `config.json`, the index files and the
  archives. A copy records its source's etag in its metadata, so an unchanged
  object isn't copied again;
- `agents/packages/<org>/<package>.json`: each package's lines as its
  `PackageIndex` holds them (the authority the index file projects);
- `agents/orgs/<org>.json`: each organisation's owner, members, block and
  trusted publishers, found through the `Directory` agent that every claim
  writes to. Token digests aren't exported: tokens are short-lived.

`GET /_status/export` reports the pass in progress and the last one finished
(times and counts only), for an operator or an uptime monitor.

**Restoring packages.** Copy the backup's objects (everything but `agents/`)
into an empty store bucket, as `seed/seed.sh` does from a directory. Each
`PackageIndex` adopts its index file on its first call, so the packages come
back with no agent state. Compare `agents/packages/` with the index files to
check nothing was lost between the last write and the export.

**Restoring organisations** is manual until R5a.2's operator routes: claim each
name for its owner as an operator (`POST /account/orgs/<org>` with `owner`),
then add its members and trusted publishers from `agents/orgs/<org>.json`.

**The backup is never pruned.** An object deleted from the store stays in the
backup, and a restore brings back everything the backup has ever held.
Archives and index files are never deleted in normal operation. A takedown
(R5a.2) deletes an archive, so it must delete the backup's copy too.

The backup bucket is in the same account: it protects against bugs and
mistakes, not against losing the account. Copy it elsewhere as well (for
example with `rclone`, from R2's S3-compatible API).

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
BYNK=$(command -v bynk) scripts/check-accounts.sh
BYNK=$(command -v bynk) scripts/check-trusted.sh
BYNK=$(command -v bynk) scripts/check-limits.sh
BYNK=$(command -v bynk) scripts/check-export.sh
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

`check-accounts.sh` serves the same empty registry with `scripts/fake-github.py`
standing in for GitHub. It signs two people in through the device flow, claims
organisations, adds a member, mints publish tokens, runs the full conformance
with an account-minted token, and checks that revoked tokens, blocked names,
reserved names and signed-out sessions are refused.

R2 and `bynk.crypto` are experimental in the standard library, so `bynk.toml`
opts in under `[unstable] allow`; this service is the workload they graduate on.
