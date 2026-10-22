---
title: Registry protocol
---
A Bynk **registry** serves packages by `organisation/package`. The protocol is
small enough that a registry can be a directory on any static file host or
object store; `bynk registry` builds one (see
[Run a private registry](/book/guides/projects-build-and-deployment/run-a-private-registry/)).
The first-party registry speaks the same protocol. This page is the
specification a registry, a mirror or a client implements.

All paths below are relative to the registry's **index URL**, the `index` of a
[`[registries]`](/docs/manifest/#registries) entry: `https://…`,
`http://localhost…`, or `file:///…` for a directory.

## `config.json`

```json
{
  "protocol": 1,
  "id": "acme",
  "dl": "dl",
  "auth": "none",
  "api": "api",
  "keys": [
    { "key": "ed25519:kFmedkdAS3yP…" },
    { "key": "ed25519:Q2hhbmdlZC…", "sig": "…" }
  ]
}
```

| Field | Meaning |
|---|---|
| `protocol` | `1`. A client refuses any other. |
| `id` | The registry's stable identity. Every signed index line binds it, so a line can't be replayed from another registry. |
| `dl` | The archive base: absolute, or relative to the index (`dl`). |
| `auth` | `none`, or `token` when requests need a bearer token. |
| `api` | Optional. The [publish API](#publish-api) base: absolute, or relative to the index. Absent for a registry published to only by `bynk registry add`. |
| `keys` | The **key-rotation chain**, oldest first. Each entry after the first carries `sig`: the *previous* key's signature over `{"key":"<this key>","registry":"<id>"}` (canonical JSON, below). |

A key is written `ed25519:<base64 of the 32-byte public key>`; its fingerprint
is `ed25519-fp:<base64 of sha256(public key)>`.

## Index files

`<org>/<package>` holds one JSON object per line, one line per version,
**append-only**:

```json
{"name":"acme/money","version":"1.4.2","checksum":"sha256-2kD/d3QS…","deps":[],"npm":[],"toolchain":null,"yanked":false,"sig":"j8udAj6L…"}
```

| Field | Meaning |
|---|---|
| `name` | `organisation/package`. |
| `version` | Semver. |
| `checksum` | `sha256-<base64>` of the package's **canonical tree** (below). |
| `deps` | Its dependencies, each `{ "package", "range" }`. A registry package depends only on registry packages. |
| `npm` | The npm packages its adapters' `requires` name, each `{ "package", "range" }`. |
| `toolchain` | The minimum `bynk` version it needs, or `null`. |
| `yanked` | Kept for locked builds; never newly chosen. The only field that may change. |
| `sig` | base64 ed25519 signature over the line's **canonical payload**. |

**The canonical payload** is a JSON object of every field except `sig` and
`yanked`, plus `"registry": "<id>"`, with object keys in byte-wise order and no
insignificant whitespace:

```json
{"checksum":"sha256-…","deps":[],"name":"acme/money","npm":[],"registry":"acme","toolchain":null,"version":"1.4.2"}
```

A client accepts a line only if a key in the trusted chain signed it, its
signed `registry` is the id it trusts for that registry, and its signed `name`
is the package it asked for (`bynk.registry.bad_signature`). Because `yanked`
isn't signed, yanking never re-signs a line — and a mirror can hide, invent or
**undo** a yank, so it can steer a *new* resolution towards a version its
registry yanked, even one yanked for a security fix. It can never change a
version's content, and a version a project has locked is pinned by checksum.

## Archives

`<dl>/sha256/<hex>` holds the version whose canonical-tree checksum has that
hex digest. An archive is the bytes `bynk-tree-v1\n` followed by each file of
the package as `path \0 length \0 bytes`, paths `/`-separated, in byte-wise
order — the very stream the canonical-tree checksum hashes. A client unpacks
only plain relative file paths (no `..`, no absolute path, no `\`, no repeats,
within size limits; `bynk.registry.bad_archive`) and admits the tree only when
its checksum is the signed one (`bynk.package.checksum_mismatch`). Build
output (`out/`), `node_modules/` and hidden files are never packed.

**The canonical tree** of a directory is every regular file except under
`.git`, in byte-wise sorted `/`-separated path order, each contributing
`path \0 length \0 bytes` to one sha256. Git and registry packages share it, and
the store names entries by it.

## Trust

A client trusts, in order: a key pinned as `key = "ed25519:…"` in
`[registries]`; else the fingerprint `bynk.lock` recorded; else, on first use,
the newest key in `config.json` (it says so, and records it). It then accepts
every key the rotation chain reaches **from** that trusted key, verifying each
link. A config whose keys don't lead from the trusted key, or that declares a
different `id` than the lock recorded, is `bynk.registry.key_mismatch`.

Rotation covers planned rotation only. Whoever holds a leaked old key can sign
a chain of their own, so recovering from a compromised key is a `[registries]`
key edit and `bynk update`.

## Auth

When a registry needs a token, the client sends `Authorization: Bearer <token>`
from `BYNK_REGISTRY_<NAME>_TOKEN` (the registry's name upper-cased, `-` as
`_`), else `[registries.<name>] token` in `~/.config/bynk/credentials.toml`.
The token goes **only** to the exact origins (scheme, host, port) of the
registry's index and archive base; redirects are followed by hand, a redirect
to another origin drops the token, and one from `https` to `http` is refused.
A token never appears in `bynk.toml`, `bynk.lock`, an archive or a message.

## Publish API

A registry with an `api` takes publishes over HTTP; `bynk publish` and
`bynk yank` use it, and `bynk registry serve` implements it. The API base obeys
the same scheme rule as `dl`: an `https` index needs an `https` API, and a
`file://` index can't have one. Every request carries
`Authorization: Bearer <token>`, found exactly as for [reads](#auth) and sent
only to the API's origin. A redirect is refused, never followed. `config.json`
isn't signed, so a publisher trusts the index's host with its token, just as a
reader trusts it with `dl`.

| Request | Effect |
|---|---|
| `PUT <api>/<org>/<package>/<version>` | The body is the version's archive (`Content-Type: application/vnd.bynk.tree-v1`). |
| `POST <api>/<org>/<package>/<version>/yank` | Marks the version yanked. |
| `POST <api>/<org>/<package>/<version>/unyank` | Clears the yank. |

The **registry signs**, not the publisher: on a `PUT` it unpacks the archive by
the client's rules, admits it only when its `bynk.toml` has a `[package]` whose
`name` and `version` are the ones the URL names and whose dependencies are all
registry dependencies, refuses a version the index already lists with a
different tree, stores the archive under `<dl>/sha256/<hex>`, and appends the
line, signed with its current key. Signing keys stay on the server; publishers
hold only tokens.

A success is `201` with the new index line for a publish, or `200` for a yank.
**A retry is idempotent:** a `PUT` of a version the index already lists, whose
upload is the very tree its line names (the same canonical-tree checksum),
answers `200` with the existing line and changes nothing. So a publish whose
`201` was lost can simply be sent again. A refusal is a status with
`{"error": "<code>", "message": "…"}`:

| Status | `error` | Meaning |
|---|---|---|
| `400` | `invalid_package` | Not a package archive, or not the package or version the URL names, or one with a path or git dependency. |
| `401` | `unauthorised` | No token, or one the registry doesn't know. |
| `403` | `forbidden` | The token can't act for the organisation, or can't do this (`publish`, `yank`). |
| `404` | `not_found` | No such endpoint, or no such version to yank. |
| `409` | `version_exists` | The version is already published with a different tree; a published version never changes. |
| `413` | `too_large` | The upload is past the registry's limit. |

A client doesn't take the response's word for it: `bynk publish` then reads the
index as any consumer would, verifying the line's signature against its trusted
key and its checksum against the tree it sent (`bynk.package.checksum_mismatch`
if they differ). A failed or refused request is `bynk.registry.publish_refused`.

## Conformance

`bynk registry conformance <index-url>` checks a registry against this page:
the reference server passes it, and any registry — a hosted one, or one you
wrote — can be run against it. A full run needs a publish token for an
organisation (`--org`); it publishes one throwaway package,
`<org>/conformance_<utc>_<hex>`, and leaves every version it published yanked
(nothing is ever deleted, so the package stays). `--read-only` publishes
nothing: it checks `config.json` and reads back an existing `--package`.

| Check | What a registry must do | Section |
|---|---|---|
| `config.shape` | Serve a `config.json` with `protocol` 1, `id`, `dl`, `keys`, and an `api` under `dl`'s scheme rule | [`config.json`](#configjson) |
| `config.chain` | Keep every link of the key chain verifiable from the trusted key | [Trust](#trust) |
| `publish.created` | Answer a new version's `PUT` with `201` and its signed line | [Publish API](#publish-api) |
| `publish.read-back` | List the line, signed by a trusted key, with the checksum of the tree sent, and serve that archive | [Index files](#index-files) |
| `publish.retry-same-tree` | Answer a same-tree retry with `200` and the existing line | [Publish API](#publish-api) |
| `publish.conflict` | Refuse a different tree at an existing version with `409 version_exists`, leaving the index unchanged | [Publish API](#publish-api) |
| `refuse.other-package`, `refuse.other-version`, `refuse.junk`, `refuse.traversal`, `refuse.path-dependency`, `refuse.clean` | Refuse an archive for another package or version, a non-archive, a traversing path, and a path dependency, each with `400 invalid_package`, and leave no line behind | [Publish API](#publish-api) |
| `auth.missing`, `auth.unknown`, `auth.scope` | `401` with no token or an unknown one; `403` for a token that can't act for the organisation (needs `--narrow-token-env`) | [Publish API](#publish-api) |
| `limits.too-large` | `413 too_large` for an upload past the cap (`--max-upload`, default 64 MiB) | [Publish API](#publish-api) |
| `consume.resolve` | Let a fresh client choose the version and admit its tree | [Index files](#index-files) |
| `yank.yank`, `yank.unyank`, `yank.missing` | Flip `yanked` with `200`, shown in the index; `404 not_found` for a missing version | [Publish API](#publish-api) |
| `errors.shape` | Give every refusal a JSON `{"error", "message"}` body with the code above | [Publish API](#publish-api) |

The redirect rule and caching aren't checked: refusing a redirect is the
client's job, and caching is the operator's choice. A failed check is
`bynk.registry.nonconformant`, naming the check and its section; `--format
json` reports every check for CI.

## Mirrors

A mirror is a copy of a registry's files — its `config.json` included —
served elsewhere. `[registries.<name>] mirror-of = "<origin>"` reads the
origin's packages from the mirror and verifies every line against the
**origin's** trust, so a mirror can withhold packages but not forge them.
