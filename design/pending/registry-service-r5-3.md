---
level: patch
changelog: "The first-party registry service has its deploy workflow (R5.3, [#2392](https://github.com/bynk-lang/bynk/issues/2392)): `deploy-registry.yml` deploys a release tag on `main` only, with that release's checked `bynk` and admission module, to staging first (held to full conformance and a real OIDC exchange) and then, after an approval, to production"
---

## ADR: registry-service-r5-3
title: The first-party registry's deploy workflow
summary: A manual workflow deploys only a released tag on `main`, with the release's own checked toolchain, to staging and then, behind an approval, to production

**Context.** Track D11 says production deploys only from release tags, with
that tag's released toolchain. E3 says no account configuration goes in git,
and E4 says deploys come only from a separate, manually triggered workflow.
`bynk deploy --env` deploys an environment and records its resources in a
committed, secret-free `bynk.deploy.lock`. No workflow deploys a Bynk service
yet. The accepted proposal is #2392, which allows release candidates on staging
only.

**Decision.**

- **A. `.github/workflows/deploy-registry.yml`,** run by hand with a `tag`
  input.
  - **The guard** (`scripts/deploy-guard.sh`, checked out from `main`) asks
    GitHub whether the tag has a published release, and whether `main` is the
    tag's commit or ahead of it. Because it asks GitHub rather than a clone,
    the answer doesn't depend on fetch depth.
  - **Release candidates.** A `-rc` tag, whose release may be a pre-release,
    deploys to staging only.
- **B. The released toolchain.** `scripts/install-release.sh` downloads the
  tag's `bynk` archive and its `bynk_registry_core` module. Each must be listed
  in, and match, the release's `SHA256SUMS`. Nothing is built from source.
- **C. Staging, then production.**
  - **`staging`** (environment `registry-staging`) runs:
    1. `bynk deploy --env staging`;
    2. a full conformance run;
    3. a trusted-publishing exchange with the job's own GitHub OIDC token.
  - **`production`** (environment `registry-production`, whose required
    reviewers are the approval) runs:
    1. `bynk deploy --env production`;
    2. the conformance read checks.
  - **Credentials.** Each step that needs them gets them, and no other step.
  - **Concurrency.** `concurrency: deploy-registry`, never cancelled.
- **D. Nothing account-specific in git.**
  - **The environments** hold every secret and variable, listed in the README.
  - **The first deploy of each environment is local** (R6). It allocates
    `bynk.deploy.lock`'s bucket names and rate-limit namespace ids.
- **E. Proof.**
  - **In CI:** the guard's decisions are tested (`scripts/test-deploy-guard.sh`:
    release tags, branches, unknown tags, drafts, pre-releases, candidates and
    tags off `main`), and the workflow passes `actionlint`.
  - **By hand:** the guard accepts the real `v0.377.2` and refuses an unknown
    tag. The installer verifies a real release's archive, and refuses a release
    without the admission module.
  - **Not yet:** the workflow's first run is part of R6.

**Deviations from the accepted proposal.**

- **Whether a tag is on `main` comes from GitHub's compare API, not
  `git merge-base`.** That answer doesn't depend on the clone's history.
- **The production job's read checks need a probe package**
  (`REGISTRY_PROBE_PACKAGE`), because `--read-only` checks one package.

**Consequences.**

- A deploy is reproducible from its tag, and auditable in the Actions log.
- Releases before the admission module was attached (`v0.377.2` and earlier)
  can't be deployed. The service's first deploy needs a release that includes
  it.
