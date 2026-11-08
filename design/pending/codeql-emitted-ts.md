---
level: patch
changelog: A nightly CodeQL scan now covers the TypeScript the compiler emits for Cloudflare Workers, with models of the Worker request and the redirect `Location` header, and deliberately vulnerable canary projects that fail the scan if those models stop firing
---

## ADR: codeql-over-emitted-typescript
title: CodeQL scans the emitted Workers TypeScript nightly, with Bynk's own models and canaries
summary: QL models of the Worker request + Location header, proved by canaries; out of band, never a gate

**Context.** A flaw in a codegen template lives only in the emitted TypeScript.
CodeQL scans the repo's own sources (`codeql.yml`), not the output, so nothing
scanned the emitted code (#255).

#255's experiment ran the stock JavaScript/TypeScript suite over every example's
output and got zero results. That zero meant nothing:

- the stock models know Express and Node, not a Cloudflare module Worker, so
  they see no user input there at all;
- the SSRF-shaped example it cited (`webhook-relay`) takes its target from
  `Secrets`, not the request, so it never tested sources either.

A scan with only the stock models would read as assurance it does not give.

**Decision.**

- **The scan.** `scripts/codeql-emitted.sh` compiles `examples/*`, plus canary
  projects, for the `workers` target. It runs the stock code-scanning suite
  plus a repo-local pack (`.github/codeql/bynk-emitted/`).
- **The models.** The pack models two things:
  - the Worker entry's `request` parameter, as a remote source;
  - a Fetch API `Response`'s `Location` header, as a URL-redirect sink.

  It re-runs `js/request-forgery` and `js/server-side-unvalidated-url-redirection`
  with those models imported, as `bynk/emitted-request-forgery` and
  `bynk/emitted-url-redirection`.
- **QL, not models-as-data.** A data-extension row can only name code by npm
  package, or by a `file:` path that another file imports. The emitted runtime
  is reached only through relative imports, and the Worker entry is imported by
  nothing.
- **Canaries.** Each deliberately vulnerable canary (SSRF through a body field
  and through `Option`; open redirect through `?next=` and through `Result`)
  must be flagged by its expected query, inside its own emitted tree, or the
  scan fails.
  - The canaries also set the models' scope. No `Fetch.send` sink and no
    `Result`/`Option` summaries were needed: default taint already follows the
    DI wiring to the global `fetch` and the `{ tag, value }` codecs.
  - A model is added only when a canary needs it.
- **Out of band.** The scan runs only on `codeql-emitted.yml`'s nightly
  schedule and on manual dispatch. It is not part of `CI green`, the merge
  queue or a release.
  - Results upload under their own category, `emitted-ts`, with canary results
    stripped.
  - A failed run uploads nothing and opens or comments on a tracking issue.

**Consequences.**

- The emitted SSRF and open-redirect paths are now under SAST, and a model
  that goes blind is noticed within a day instead of passing silently.
- **Coverage is narrow.** Only the two wrapped queries see user input; every
  other stock taint query stays as blind as before. Covering another class
  means wrapping its query and adding a canary for it.
- **Stored flows are out of scope.** CodeQL does not follow taint across KV.
  `link-shortener`'s user-stored redirect target is that example's purpose, so
  a `Kv.get` source would only flag it forever.
- **Alerts point at generated code** that isn't in the tree. The fix belongs in
  the emitter or runtime template.
- This does not replace ADR 0087's behavioural tests of the emitted boundaries.
  Those stay the load-bearing guard where Bynk controls the semantics; this
  adds breadth over the two classes it models.
