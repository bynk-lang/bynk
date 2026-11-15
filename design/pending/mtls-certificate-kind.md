---
level: minor
changelog: A linked `MtlsCertificate` kind, `capability Backend = MtlsCertificate @certificate("acme-client")`, whose `send` presents an account-held client certificate to an origin (experimental; the deploy lands separately)
probes: diagnostic_coverage, emit_diagnostics, emit_abi_shapes, ts_writes
---

## ADR: mtls-certificate-linked-kind
title: `MtlsCertificate`, a linked instance kind that presents an account-held client certificate
summary: Named, never uploaded or given by id; `send` has Fetch.send's shape; remote-only under `bynk dev`; faults until stubbed

**Context.** Cloudflare's `mtls_certificates` binding makes a Worker present a client
certificate when it calls an origin that requires mutual TLS. The binding is a `Fetcher`
whose `fetch()` has the global fetch's signature and always presents the certificate; the
certificate is uploaded to the account (with its private key) and the Worker config names
it by id. Proposal #3513 Part A, accepted. The facts were checked against Cloudflare's
documentation ([mTLS binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/mtls/),
[Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/#mtls-certificates),
[local development](https://developers.cloudflare.com/workers/local-development/),
[the list endpoint](https://developers.cloudflare.com/api/resources/mtls_certificates/methods/list/)),
and all held; the checks added the details marked below.

**Decision.**

- **A1: a linked instance kind.** `MtlsCertificate` is an `Instantiable` with the required
  option `certificate` (a string), binding prefix `MTLS`, instance-only and `linked`: the
  third user of the flag after `StoredSecret` and `AiSearch` (ADR 0802). The certificate is
  **named**, never given by id, and the name is used as written in every environment. An
  empty name, or one with a control character, is `bynk.capability.invalid_option`; a bare
  `given MtlsCertificate` is `bynk.capability.instance_only`.
- **A3: `send` first.** `send(req) -> Effect[Result[Response, FetchError]]` has `Fetch.send`'s
  request, response and `FetchError` rules, verbatim, over the binding's `fetch`
  (`WorkersMtls`, sharing `WorkersService`'s request path). A handshake the origin refuses is
  `Network`; `FetchError` is not widened. The `Bytes` forms follow, additively, with the deploy.
- **A5: tests fault until stubbed.** A call that presents a real certificate to a real origin
  has no honest fake, so every operation throws, naming the instance, until
  `stub Backend.send(_) returns …`.
- **A6: experimental, locked by consuming.** A use needs `[unstable] allow`;
  `consumes bynk.cloudflare { MtlsCertificate }` locks the unit to Cloudflare (ADR 0017), and
  per-file `consumes` applies (ADR 0849).
- **Emission.** `[[mtls_certificates]] binding, certificate_id`, with the certificate's name in
  `certificate_id` and a comment saying `bynk deploy` replaces it with the id it resolves.
- **A4 (remote-only under `bynk dev`) is confirmed** by Cloudflare's local-development page:
  "There is no current local simulation for mTLS bindings", `remote = true` goes on the stanza,
  and Wrangler warns when it is omitted. The `bynk dev` acknowledgement (ADR 0834) lands with
  the deploy slice.
- **Additions from the verification, none contradicting a decision.** The binding cannot be
  used for a service that is a proxied zone on Cloudflare (Cloudflare answers `520`), so the
  doc comment and the reference say so. The list endpoint's records carry an optional `name`,
  may be CA certificates (`ca: true`, no private key, not presentable by a Worker), and are
  paged; the deploy slice's resolver accounts for each. The permission the lookup needs is
  "Account: SSL and Certificates Read".

**Consequences.** A program that does not consume `MtlsCertificate` emits byte-identically
apart from the standard library's `cloudflare.ts`. Until the deploy slice lands, `bynk deploy`
cannot resolve the certificate name to an id, so it refuses a Worker that declares an
`MtlsCertificate` before anything is pushed, rather than send Cloudflare a name where it expects
an id; the slice that adds the resolver removes the refusal. Per-environment
certificate names are not in v1 (A7); a `[deploy.mtls.<Cap>]` override is the follow-on.
