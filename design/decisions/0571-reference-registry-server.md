# 0571 — `bynk registry serve` is a single-threaded, loopback-by-default reference server with hashed, scoped tokens

- **Status:** Accepted (v0.385)

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
restart. Because requests are handled one at a time, a slow upload delays every other request, reads included,
until it completes or times out — acceptable for a reference server, not for a busy registry. Uploads are staged
in a fresh, private (`0700`) directory created exclusively under a random name, so no other local user can
swap a file between unpacking and signing. 4b's accounts replace the tokens file for the first-party registry;
the API is the same.
