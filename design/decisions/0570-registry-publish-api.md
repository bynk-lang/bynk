# 0570 — A registry takes publishes over HTTP and signs them itself; the client verifies what it signed

- **Status:** Accepted (v0.385)

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
this API unchanged; only how its tokens are issued differs. `config.json` is unsigned, so whoever controls the index's `config.json`
decides where the publish token goes (any origin `dl`'s scheme rule allows), exactly as it already decides
where read tokens go through `dl`; a publisher trusts the index host with its token. A same-origin rule was
considered and left out, so that an API can sit on its own host behind its own proxy.
