---
title: Deploy from CI
---

Deploy a Bynk project from GitHub Actions with the
[`bynk-lang/bynk-deploy`](https://github.com/bynk-lang/bynk-deploy) action. It
installs Bynk and Node, then runs [`bynk deploy --yes`](/reference/tooling/bynk-cli/#bynk-deploy)
for you. This guide covers what CI changes about a deploy and how to set it up;
the action's own README documents every input, so this page names the ones it
uses and does not restate the rest.

## Deploy locally first

A deploy from CI cannot create everything. A KV namespace and a Hyperdrive
config get an id from Cloudflare, and a rate limiter's namespace id is
allocated by the deploy ledger; each id must be written to `bynk.deploy.lock` and
committed, and a CI run has no way to commit back to the branch it deployed.
So when `CI` is set (GitHub sets it) `bynk deploy` refuses to make one, and
names the resource:

```text
bynk: KV namespace for `<worker>` is unrecorded; provision locally first and commit bynk.deploy.lock
```

Run the first deploy of each environment from your own machine, then commit the
lock file:

```sh
bynk deploy
bynk deploy --env staging
git add bynk.deploy.lock
git commit -m "Record the deploy state"
```

After that, CI can push every later build. It can still create queues, R2
buckets, D1 databases and Analytics Engine datasets, declare Durable Objects
and set secrets, because their identity is a name from your source. A Hyperdrive
config can also be named by id (`config_id` in `[deploy.hyperdrive]`), so a team
that deploys only from CI never needs one minted. See
[Deploy to Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#what-gets-provisioned).

When a later deploy does change the lock (a new queue is recorded, say), the
action sets its `lock-changed` output to `"true"`. Give it `lock-pr-token`, a
GitHub App installation token, and it opens a pull request that commits the
change; its README has the setup.

## The workflow

```yaml
name: Deploy
on:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  deploy:
    runs-on: ubuntu-latest
    environment: production
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: bynk-lang/bynk-deploy@422b47aeb37ba7ad0d8ba1f87755ae7e422dc462 # v2.2.2
        with:
          version-file: BYNK_VERSION
          environment: production
          cloudflare-api-token: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          cloudflare-account-id: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
```

`version-file` reads an exact Bynk version from a file in the repository, so the
version that deploys is the one you committed. Pin the action to a commit, as
here. The action passes `--yes`, because a CI job has no one to ask.

## The token

`CLOUDFLARE_API_TOKEN` is the account's API token. A dry run never
authenticates, but a real deploy needs a token that can deploy Workers and
manage what the project declares. These are the permissions this guide's
resources name; each lookup checks Cloudflare before the deploy changes
anything, and a refusal names the permission it lacked.

| Resource | Permission | Used by |
|---|---|---|
| Hyperdrive config (`Postgres`) | Hyperdrive: Edit | Creating the config from its connection string |
| Secrets Store secret | Secrets Store: Read | Checking the stored secret exists, before the deploy |
| AI Search instance | AI Search: Edit and AI Search: Run | Checking the instance exists (Cloudflare names no read-only permission) |
| mTLS certificate | SSL and Certificates: Read | Checking the certificate exists |
| Email Routing forward destinations | Email Routing Addresses: Read (Write also works) | Checking a literal forward address is verified; skipped with a warning without it |
| Email Sending domains | Read access to the zone and its Email Sending settings | Checking a sender domain is enabled |

The token also needs the account permissions to upload Workers and create the
resources your project declares (Cloudflare's token documentation lists them
per product); this guide names only the ones the deploy guide records. Keep the
token in a repository or environment secret, and scope it to the one account.

## Secrets

Pass the values your Workers read as dotenv lines in the action's `secrets`
input, each taken straight from a repository or environment secret:

```yaml
      - uses: bynk-lang/bynk-deploy@422b47aeb37ba7ad0d8ba1f87755ae7e422dc462 # v2.2.2
        with:
          version-file: BYNK_VERSION
          environment: production
          cloudflare-api-token: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          cloudflare-account-id: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
          secrets: |
            AUTH_JWT_SECRET=${{ secrets.AUTH_JWT_SECRET }}
            STRIPE_KEY=${{ secrets.STRIPE_KEY }}
```

The action masks each value, writes the lines to a temporary file readable only
by the job, passes it as `bynk deploy --secrets-file`, and deletes it afterwards,
even after a failure. That is also why you give secrets to `--secrets-file` and
not to wrangler after `--`: a run that includes a new Worker refuses
`-- --secrets-file`, because that upload already passes its own file.

Pass only values that come directly from `secrets.*`: GitHub masks those in the
log, but prints a literal, or a value built from a secret, in the step's header.
A name that is declared but unset fails the deploy, since CI has no prompt, and
a secret that is already set is left alone unless you pass `force-secrets: "true"`
after rotating it. See [Secrets](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/#secrets).

## A plan on pull requests

`bynk deploy --dry-run --format json` prints the plan without changing
Cloudflare or the lock, and needs no token. Run it on every pull request and put
the plan in the job summary:

```yaml
name: Deploy plan
on:
  pull_request:

permissions:
  contents: read

jobs:
  plan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: bynk-lang/bynk-deploy@422b47aeb37ba7ad0d8ba1f87755ae7e422dc462 # v2.2.2
        id: plan
        with:
          version-file: BYNK_VERSION
          environment: production
          dry-run: "true"
      - env:
          PLAN: ${{ steps.plan.outputs.plan }}
        run: |
          {
            echo '### Deploy plan'
            echo '```json'
            echo "$PLAN" | jq .
            echo '```'
          } >> "$GITHUB_STEP_SUMMARY"
```

The `plan` output is always JSON. Without a token the plan is built offline, so
it shows what the project would create rather than what is already on the
account; give the step a read-capable token if you want it to mark existing
resources as reused (see the `--dry-run` row of the
[CLI reference](/reference/tooling/bynk-cli/#bynk-deploy)).

## Environments

Deploy one environment per job, with `environment:` naming the `--env` it
deploys to, and use GitHub's environment protection rules on the one that
matters. A job that names a GitHub environment waits for its required
reviewers before it runs, and reads that environment's secrets, so staging and
production keep separate tokens:

```yaml
jobs:
  staging:
    runs-on: ubuntu-latest
    environment: staging
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: bynk-lang/bynk-deploy@422b47aeb37ba7ad0d8ba1f87755ae7e422dc462 # v2.2.2
        with:
          version-file: BYNK_VERSION
          environment: staging
          cloudflare-api-token: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          cloudflare-account-id: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
  production:
    needs: staging
    runs-on: ubuntu-latest
    environment: production # add required reviewers in the repository's settings
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      - uses: bynk-lang/bynk-deploy@422b47aeb37ba7ad0d8ba1f87755ae7e422dc462 # v2.2.2
        with:
          version-file: BYNK_VERSION
          environment: production
          cloudflare-api-token: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          cloudflare-account-id: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
```

Each environment keeps its own section of `bynk.deploy.lock`, so run the first
deploy of each locally, as above. The action can also smoke-test a deploy
(`smoke`) and, experimentally, revert a failed smoke; see its README.

## Why `--prune` stays manual

`bynk deploy --prune` deletes what the ledger remembers but the build no longer
declares, a KV namespace or a queue, and it is also what lets a deploy go
ahead after an Email Routing address has been removed. Deleting a namespace
loses its data and cannot be undone, and with `--yes` no one is asked, so the action has no input for it. `--yes` answers the creation
confirmation but never stands in for an agent-class `--confirm-delete`. Review the
orphans in the plan, then run `bynk deploy --prune` yourself, where it asks for
its own confirmation.
