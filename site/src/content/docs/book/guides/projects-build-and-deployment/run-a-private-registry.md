---
title: Run a private registry
---
**Goal:** publish your organisation's packages to a registry only you serve,
and depend on them by version.

A Bynk registry can be a plain directory: `bynk registry` builds it, any
static file host or object store serves it, and every package in it is signed
and checksummed. Nothing runs on the server. When publishers shouldn't hold the
signing key, [put a publish API in front of it](#publish-over-http) instead.

## Create the registry

```bash
bynk registry init ./registry --id acme --key-out ~/secrets/acme.key
```

This writes `./registry/config.json` and a new signing key. It prints the line
your users pin:

```text
key = "ed25519:kFmedkdAS3yPsVDgTy4imVobJ6VdB30GLQUJU7izElY="
```

Keep the key file secret, and **outside** the registry directory: the
directory is what you publish. Anyone with the key can sign packages as your
registry.

## Add a package

A package needs a `[package]` with a `version`, and depends on other packages
only by `version` (a registry refuses `path` and `git` dependencies):

```bash
bynk registry add ./registry ./packages/money --key ~/secrets/acme.key
```

This packs the package's sources (never `out/`, `node_modules/` or hidden
files), signs its index line and appends it. A version can be added once: bump
`version` to publish a change.

## Serve it

Upload `./registry` to any static host: a web server, an object-store bucket
behind HTTPS, an internal file share. To keep it private, put it behind a
host that checks `Authorization: Bearer <token>` and set
`"auth": "token"` in `config.json`. For an air-gapped machine or a quick test,
skip hosting and point at the directory as `file:///…`.

## Use it

In each project (or once, at the workspace root), route your organisation to
the registry and pin its key:

```toml
[registries.acme-internal]
index = "https://bynk.acme.internal/registry"
organisations = ["acme"]
key = "ed25519:kFmedkdAS3yPsVDgTy4imVobJ6VdB30GLQUJU7izElY="
```

Then add a dependency — `bynk add` picks the newest version and writes the
entry for you:

```bash
bynk add acme/money
```

```toml
[dependencies]
money = { package = "acme/money", version = "^1.4.2" }
```

`bynk check` (or `test`, `dev`, `deploy`, `fetch`) chooses the highest version
every range allows, verifies its signature and checksum, fetches it into the
store and records it in `bynk.lock`. Commit the lock. A token, if the registry
needs one, comes from `BYNK_REGISTRY_ACME_INTERNAL_TOKEN` or
`~/.config/bynk/credentials.toml` — never from `bynk.toml`.

## Yank a version

```bash
bynk registry yank ./registry acme/money 1.4.2
```

A yanked version is never chosen again, but a build that already locked it
keeps working. Nothing is deleted.

## Withdraw a version

Yank a version that is merely broken or superseded. **Withdraw** one that must
not be served at all — a legal takedown, confirmed malware:

```bash
bynk registry withdraw ./registry acme/money 1.4.2 --reason "removed at the copyright holder's request (case 2026-114)"
```

This yanks the line, records the reason on it (one line, at most 500
characters), and deletes the version's archive. If another version is the
identical tree, the shared archive is kept and the command warns you. Nothing
is re-signed. If the command fails after it has marked the line (say, the archive
couldn't be deleted), run it again: it finishes the deletion. Like `bynk registry
yank`, it rewrites the index from outside any server, so on a directory behind
`bynk registry serve`, run it when no publish is in flight — a publish landing
mid-run can lose its line. A withdrawal is final: there is no `--undo`, the registry refuses
to publish that version again (even the same tree) or un-yank it, and a version
that comes back is published under a new version number.

A project whose `bynk.lock` names the version keeps building on a machine that
already has its tree. Anywhere else the build fails as
`bynk.registry.withdrawn`, quoting your reason and suggesting
`bynk update <alias>` to move to another version. Run
`bynk registry conformance --read-only --package acme/money` to check the
registry's withdrawals are consistent (`read.withdrawn`).

## Publish over HTTP

`bynk registry add` needs the signing key on the machine that publishes. To
keep the key on one server and give publishers only tokens, serve the
directory with its publish API:

```bash
bynk registry token ~/secrets/tokens.toml --org acme
bynk registry serve ./registry --key ~/secrets/acme.key --tokens ~/secrets/tokens.toml
```

`token` prints a token once — hand it to the publisher (or CI) — and keeps only
its hash. Add `--can publish` for a token that can't yank. `serve` listens on
`127.0.0.1:8730`, serves the directory's files, and takes publishes at `/api`.
Keep both files outside the directory. To serve beyond the machine, put an
HTTPS proxy in front and give consumers its address as the `index`.

A publisher then runs, in a package whose `[registries]` routes `acme` to the
registry:

```bash
BYNK_REGISTRY_ACME_INTERNAL_TOKEN=bynk_… bynk publish
```

It checks the package, uploads it, and verifies the line the registry signed.
Try `bynk publish --dry-run` first to see what would go up. Yank through the
API with `bynk yank acme/money 1.4.2`.

## Check it

To check that a registry keeps the protocol — after changing your setup, or
when you run your own server — point the conformance suite at it with a token
for an organisation it can publish under:

```bash
BYNK_REGISTRY_CONFORMANCE_TOKEN=bynk_… bynk registry conformance https://bynk.acme.internal/registry --org acme
```

Each check passes, fails or is skipped, with the protocol section it holds the
registry to. A run publishes one throwaway package and leaves it yanked; use
`--read-only` to publish nothing.

## Rotate, delegate and revoke keys

To replace the key in a planned way, rotate it:

```bash
bynk registry rotate ./registry --key ~/secrets/acme.key --key-out ~/secrets/acme-2.key
```

Clients that trust the old key follow the chain to the new one and keep
verifying older versions. A **leaked** chain key can't be rotated away —
whoever has it can sign a chain too — so tell your users to pin the new key in
`[registries]` and run `bynk update`.

To make a leak recoverable, keep the chain key offline and sign with a
**delegate** instead:

```bash
bynk registry delegate ./registry --key ~/secrets/acme.key --delegate-out ~/secrets/online-1.key
bynk registry serve ./registry --key ~/secrets/online-1.key --tokens ~/secrets/tokens.toml
```

If the delegate leaks, replace it without your users changing anything. Do it
in this order, so every line stays verifiable throughout:

```bash
bynk registry delegate ./registry --key ~/secrets/acme.key --delegate-out ~/secrets/online-2.key
bynk registry resign ./registry --key ~/secrets/online-2.key
bynk registry revoke ./registry ed25519:<online-1 key> --key ~/secrets/acme.key
```

Publish `config.json` after `delegate` and again after `revoke`. The chain key
only ever signs delegations and revocations, so it can stay on an offline
machine: the commands work on a copy of the registry's `config.json`.

## Related

- [Registry protocol](/docs/registry-protocol/) — what a registry serves.
- [`[registries]`](/docs/manifest/#registries) and
  [`[dependencies]`](/docs/manifest/#dependencies).
- [Share code between projects](/book/guides/projects-build-and-deployment/share-code-between-projects/).
