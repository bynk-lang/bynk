---
level: minor
changelog: `bynk deploy` links each `MtlsCertificate` by name, writes the certificate's id into the Worker's configuration and records it in the ledger, failing when it is missing or ambiguous and never uploading one; `bynk dev` runs the binding on Cloudflare, with a one-time notice silenced by `[dev] mtls = "allow"`; the byte forms `receiveBytes` and `sendBytes` join `send`
probes: diagnostic_coverage, ts_writes
---

## ADR: mtls-certificate-deploy-link
title: `bynk deploy` links an mTLS certificate by name and never uploads it; `bynk dev` runs it remotely
summary: Resolve the name over the REST list, match client certificates only, record a name and an id, never key material; remote-only under `bynk dev`

**Context.** The `MtlsCertificate` kind (ADR for the kind slice) names a client certificate in the
account and leaves the deploy to resolve the name to the id Cloudflare's `[[mtls_certificates]]`
stanza takes. Proposal #3513 Part A, decisions A2, A3 and A4, checked against Cloudflare's
documentation ([the list endpoint](https://developers.cloudflare.com/api/resources/mtls_certificates/methods/list/),
[local development](https://developers.cloudflare.com/workers/local-development/)).

**Decision.**

- **A2: the deploy resolves by name over the REST list and never uploads.** Before anything is
  pushed, `bynk deploy` lists `GET /accounts/{id}/mtls_certificates` (paged, once per run), matches
  the record's `name`, writes the id into the Worker's `certificate_id` (the stanza carried the name
  until then) and records the link in a new ledger section, `mtls_certificate_links` (environment,
  Worker, binding to the certificate's name and id; additive and omitted when empty). Only names and
  ids cross the wire: key material is never requested, written or logged, which is why a certificate
  is linked and not created. `--prune` never touches a link.
- **Matching.** A record's `name` is optional, so a record with none cannot be linked. The list also
  holds CA certificates (`ca: true`, no private key), which a Worker cannot present, so a name
  matches **client certificates only**; a name carried only by a CA is its own message, and a CA beside
  one client certificate of the name is not an ambiguity. Two client certificates with one name fail
  the deploy naming both ids. A missing certificate fails it with where to upload one (Wrangler's
  `mtls-certificate upload`, the dashboard or the API) and that the deploy will not. Every problem is
  reported together, before anything is pushed.
- **Expiry is a warning.** A certificate that has expired, or expires within 30 days, is a warning and
  not a failure: a Worker bound to an expired certificate deploys fine and fails at the handshake, so
  the deploy is the useful moment, and a deploy that fixes nothing is not blocked by a certificate the
  Worker may not call yet. The 30-day window is this record's choice; the proposal named "soon-expiring"
  without a number.
- **The plan line** is `will link mtls-certificate acme-client (MTLS_BACKEND; not created; not checked
  in a dry run)`, the wording AI Search settled on (the plan is built offline and prints the same line
  on a real run, before the lookup): it never says "linked" before the lookup has succeeded.
- **Token.** The lookup needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, the token with
  "SSL and Certificates: Read" (Cloudflare's list endpoint accepts the read or write form); a refusal
  names it.
- **A3: the byte forms.** `receiveBytes(req)` and `sendBytes(req, body)` join `send`, additively, as in
  `Container`: `Fetch`'s byte forms over the binding's `fetch`, both answering `BytesResponse`. (The
  proposal's sketch had `sendBytes` answer `Response`; the decision text says "as in Container", whose
  `sendBytes` answers `BytesResponse`, as `Fetch`'s does.)
- **A4: remote-only under `bynk dev` (ADR 0834).** `[dev] mtls = "allow"` is a new acknowledgement
  (`BYNK_DEV_ACK_MTLS`). The provider warns once on the first call: an mTLS binding has no local
  simulation, so the call runs on Cloudflare, presenting the account's certificate to the origin, with
  a link to Cloudflare's page and no word of price. The development copy of the config marks each
  `[[mtls_certificates]]` entry `remote = true` and binds it by the id the last deploy recorded for the
  environment. A certificate no deploy has linked is reported and its entry is dropped from the
  development copy, not left `remote = true` with a name where an id belongs (wrangler sets up its
  remote proxy for every such binding at startup, so a bad id might fail the whole start), so the
  Worker serves with the binding absent and a call through it failing. The config `bynk deploy` reads never carries `remote`.
- `[env.*]` blocks copy the stanza as written (the certificate's id is already materialised into the
  top level before the block is synthesised), with no environment suffix.

**Consequences.** `bynk deploy` of a Worker that declares an `MtlsCertificate` now needs a token with
the read permission and the certificate uploaded; before this, such a Worker's deploy was refused
outright (the kind slice shipped the refusal as a stand-in, which this removes). Per-environment
certificate names stay out of v1 (A7). The ledger gains one additive section, so an older ledger still
reads and a project without mTLS writes the same file.
