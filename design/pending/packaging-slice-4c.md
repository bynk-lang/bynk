---
level: minor
probes: diagnostic_coverage
changelog: Packaging slice 4c — publishing over HTTP. A registry's `config.json` may name a publish `api`; `bynk publish` checks and packs a package, uploads it there with the registry's token, and verifies the line the registry signed by reading it back, and `bynk yank` yanks or un-yanks through the same API (`bynk.registry.publish_refused` carries a refusal's reason). `bynk registry serve` puts the API in front of a registry directory, signing uploads with a key publishers never hold and authorising them with hashed tokens from `bynk registry token`, scoped to organisations and to `publish` or `yank`
---

## ADR: registry-publish-api
title: A registry takes publishes over HTTP and signs them itself; the client verifies what it signed
summary: Packaging slice 4c — `config.json` `api` (same scheme rule as `dl`); `PUT <api>/<org>/<package>/<version>` with the archive, `POST …/yank|unyank`; the server admits by `bynk registry add`'s rules and signs with its own key; JSON `{error, message}` refusals; `bynk publish` checks before and verifies by read-back after; `bynk yank` confirms by read-back

**Context.** Slice 4a (#2274) left publishing to `bynk registry add`, which needs the registry's signing key
and write access to its directory: every publisher could sign anything as the registry, and two appending at
once could corrupt an index. The hosted first-party registry (4b) and any organisation that wants publishing
without sharing its key need an API. Proposal #2287, decisions A, B, C and F.

**Decision.**

- **`api` (C).** `config.json` may carry `api`, absolute or relative to the index. A client accepts it only
  under `dl`'s rule — an `https` index needs an `https` API, an `http://localhost` index allows `http` or
  `https`, a `file://` index allows none — else the registry is `bynk.registry.unavailable`. The token goes to
  the API's origin too, and only to it; a redirect from the API is refused, never followed.
- **The endpoints (A).** `PUT <api>/<org>/<package>/<version>`, the body a `bynk-tree-v1` archive;
  `POST <api>/<org>/<package>/<version>/yank` and `/unyank`. Each carries `Authorization: Bearer <token>`.
- **The server signs (A).** It unpacks the upload by the client's hardened rules, requires a `[package]` whose
  `name` and `version` are the URL's and only registry dependencies, refuses an existing version, writes the
  archive before rewriting the index through a temporary file and a rename, and signs the line with its own
  current key. A publish answers `201` with the line; a refusal is `400 invalid_package`,
  `401 unauthorised`, `403 forbidden`, `404 not_found`, `409 version_exists` or `413 too_large`, each with
  `{"error", "message"}`. `bynk registry add` and the server share this admission code. Client-signed uploads
  were rejected: they need per-publisher keys in a trust model that trusts one chain per registry.
- **`bynk publish` (B).** It resolves and checks the package (one that doesn't check isn't published), refuses
  one without a `version` or with a `path`/`git` dependency, packs it, and uploads it to the registry its
  organisation routes to, with that registry's ordinary token. It then reads the index back through the
  verifying client and refuses a line whose checksum isn't the tree it sent
  (`bynk.package.checksum_mismatch`). `--dry-run` stops before the upload. A registry without an `api` points
  to `bynk registry add`.
- **`bynk yank` (F).** It routes like `publish`, calls the yank API (`--undo` un-yanks), and confirms by reading
  the index back. Nothing is deleted or re-signed.
- A failed or refused request is `bynk.registry.publish_refused`, carrying the server's code and message; a 401
  names the token variable to set.

**Consequences.** Publishers hold tokens, not keys: a leaked token can publish new versions under its own
organisations and nothing else, and a server that signs anything but what was sent is caught at once. 4b runs
this API unchanged; only how its tokens are issued differs.

## ADR: reference-registry-server
title: `bynk registry serve` is a single-threaded, loopback-by-default reference server with hashed, scoped tokens
summary: Packaging slice 4c — `tiny_http`; serves the directory by `GET` and the API at `/api` (recorded in `config.json`); key and tokens files refused inside the directory; one request at a time serialises writes; plain HTTP on `127.0.0.1:8730` by default, with a warning elsewhere; `tokens.toml` holds sha256 digests scoped to organisations and `publish`/`yank`, compared in constant time; `bynk registry token` mints one and prints it once

**Context.** A private registry that wants the publish API needs something to run it. Proposal #2287,
decisions D and E.

**Decision.**

- **The server (D).** `bynk registry serve <dir> --key <file> --tokens <file> [--listen <addr>]` serves the
  directory's plain relative files by `GET` (nothing outside it, no other method) and the API under `/api`,
  recording `"api": "api"` in `config.json` when it is missing. The key must be the registry's current key. It
  handles one request at a time, so writes to a registry never interleave. It speaks plain HTTP, binds
  `127.0.0.1:8730` by default, and warns when bound beyond loopback that tokens travel in clear unless a
  TLS-terminating proxy is in front. Uploads are capped at 64 MiB. `tiny_http` (MIT/Apache-2.0) is the one
  new crate, in the `bynk` CLI only; a hand-parsed server was rejected.
- **Tokens (E).** The tokens file holds `[[token]]` entries of `sha256` (the token's hex digest, never the
  token), `organisations` and `can` (`publish`, `yank`). A request's bearer token is hashed and compared in
  constant time: unknown is `401`, a wrong organisation or capability `403`.
  `bynk registry token <file> --org <org>… [--can publish,yank]` mints a random `bynk_…` token, appends its
  hash (writing the file private), and prints the token once. The key file and the tokens file are each
  refused inside the served directory.

**Consequences.** A private registry can take publishes without handing out its key, and 4a's "one publisher
at a time" caveat is gone for publishes through the server. Tokens are read at start, so a new one needs a
restart. 4b's accounts replace the tokens file for the first-party registry; the API is the same.
