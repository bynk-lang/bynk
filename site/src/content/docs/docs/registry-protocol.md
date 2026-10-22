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
registry dependencies, refuses a version the index already lists, stores the
archive under `<dl>/sha256/<hex>`, and appends the line, signed with its
current key. Signing keys stay on the server; publishers hold only tokens.

A success is `201` (publish) or `200` (yank) with a JSON body — the new index
line for a publish. A refusal is a status with
`{"error": "<code>", "message": "…"}`:

| Status | `error` | Meaning |
|---|---|---|
| `400` | `invalid_package` | Not a package archive, or not the package or version the URL names, or one with a path or git dependency. |
| `401` | `unauthorised` | No token, or one the registry doesn't know. |
| `403` | `forbidden` | The token can't act for the organisation, or can't do this (`publish`, `yank`). |
| `404` | `not_found` | No such endpoint, or no such version to yank. |
| `409` | `version_exists` | The version is already published; a published version never changes. |
| `413` | `too_large` | The upload is past the registry's limit. |

A client doesn't take the response's word for it: `bynk publish` then reads the
index as any consumer would, verifying the line's signature against its trusted
key and its checksum against the tree it sent (`bynk.package.checksum_mismatch`
if they differ). A failed or refused request is `bynk.registry.publish_refused`.

## Mirrors

A mirror is a copy of a registry's files — its `config.json` included —
served elsewhere. `[registries.<name>] mirror-of = "<origin>"` reads the
origin's packages from the mirror and verifies every line against the
**origin's** trust, so a mirror can withhold packages but not forge them.
