# 0603 — Trusted publishing for the first-party registry

- **Status:** Accepted (v0.399.1)

**Context.** After R3, CI publishes with a long-lived publish token held as a CI
secret. If that secret leaks, it can publish for up to a year. Track §5 promises
trusted publishing instead: a CI job publishes with its OIDC token, bound to the
repository and workflow the organisation configured. L5 (#2380) made this
possible: an `Oidc` handler reads verified claims (`who.claims`), and
`trustFrom` points the actor at a fake issuer in tests. The accepted proposal is
#2381.

**Decision.**

- **A. The exchange.**
  - `POST /account/trusted/exchange {org}` is guarded by a `Workload` actor
    (`Oidc`, with issuer `https://token.actions.githubusercontent.com` and
    audience `bynk-registry`).
  - It reads the claims `repository_id`, `job_workflow_ref` and `environment`.
  - It looks for a trusted publisher of `org` that matches the repository id,
    the workflow file, and the environment (when one is registered). A match
    mints a publish token through R3's minting that lasts 15 minutes and is
    listed as `trusted: <repository>@<ref>`.
  - When several publishers match, one bound to the job's environment wins over
    one that takes any, so the result doesn't depend on registration order.
  - **A token is exchanged once.** The repository's index records each
    exchanged token's `jti` for a day, far longer than a GitHub Actions OIDC
    token lives, and refuses it again, as PyPI's trusted publishing does. A
    token with no `jti` is refused.
  - Refusals:
    - no match, a replayed token or a token without a `jti`: `403 not_trusted`,
      naming what was checked;
    - a blocked organisation: `403 blocked`;
    - an unverified token, including one for another audience or one that has
      expired: `401`.
- **B. Trusted publishers.**
  - **Registering** is owner-only. `POST /account/orgs/<org>/trusted
    {repository, workflow, environment?, can}` resolves the repository to
    GitHub's numeric id (`GET /repos/:owner/:repo`), which is what an exchange
    matches. A repository that is renamed, deleted and recreated, or
    transferred, therefore doesn't inherit trust.
  - **Storage.** A publisher is listed on `Organisation(org)` and indexed in a
    `TrustedRepository(repositoryId)` agent, so an exchange finds its
    candidates in one call.
  - **Write order.** Registering lists the publisher first, then indexes it.
    Removing un-indexes it first, then unlists it. An exchange also checks
    that the publisher is still listed. So a half-done write never leaves an
    unlisted publisher able to mint.
  - **Validation.** The workflow must be a `.yml` or `.yaml` file directly in
    `.github/workflows/`, with no `..`.
  - **Removal is owner-only**, refused as registration is (`403`). Tokens a
    removed publisher already minted run out within their 15 minutes.
- **C. Short-lived tokens don't accumulate.** `Organisation.issue` drops lapsed
  tokens from the list as it adds one. `Accounts.mint` now takes a lifetime in
  milliseconds; the account API still offers 1–365 days.
- **D. The workflow side is documented.** The README gives a complete GitHub
  Actions job: `id-token: write`, the OIDC request, the exchange, and `bynk
  publish`. It recommends a protected environment. A client flag that does all
  of this (`bynk publish --trusted`) is a follow-on.
- **E. Proven end to end.** `scripts/check-trusted.sh` runs in CI against a fake
  issuer (`scripts/fake-oidc.mjs`, Node WebCrypto RS256), set through
  `REGISTRY_OIDC_TRUST`, and R3's fake GitHub (which now has `/repos`). It:
  - registers a publisher;
  - exchanges a matching token;
  - passes all 23 checks of the full `bynk registry conformance` run with the
    exchanged token;
  - checks that each of these is refused: another workflow, another repository
    id, another environment, another audience, an expired token, a malformed
    token, a replayed token, a blocked organisation, and a removed publisher.

  Unit tests cover the matching rules, using L5's `TokenClaims.of`.

**Deviations from the accepted proposal.**

- **The trusted-publishing routes are a separate `trusted` service** in
  `src/registry/store/trusted.bynk`, still under `/account/…`, not part of the
  `account` service. When a handler's `Oidc` identity type is declared in
  another file of a split context, the compiler doesn't import that type into
  the handler's module (a compiler gap, reported separately). Keeping the
  actor, its identity type and the exchange in one file avoids it.
- **An expired token gets the verifier's 60-second clock-skew allowance.** This
  is the `Oidc` seam's existing behaviour, not something new here.

**Consequences.**

- Releases can run from CI with no long-lived credential.
- The protocol and the client are unchanged.
- Other CI providers (GitLab and others) can be added later the same way: an
  actor with that provider's issuer, and a publisher shape for its claims.
