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

## Rotate the key

To replace the key in a planned way, append the new key to `config.json`'s
`keys`, signed by the old one. Clients that trust the old key follow the chain
to the new one and keep verifying older versions. A **leaked** key can't be
rotated away — whoever has it can sign a chain too — so tell your users to pin
the new key in `[registries]` and run `bynk update`.

## Related

- [Registry protocol](/docs/registry-protocol/) — what a registry serves.
- [`[registries]`](/docs/manifest/#registries) and
  [`[dependencies]`](/docs/manifest/#dependencies).
- [Share code between projects](/book/guides/projects-build-and-deployment/share-code-between-projects/).
