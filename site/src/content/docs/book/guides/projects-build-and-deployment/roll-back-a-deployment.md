---
title: Roll back a deployment
---
**Goal:** make a Worker's previous deployment live again after a bad release,
without a rebuild, and understand when Bynk refuses to.

## Why a platform rollback is not enough

Cloudflare can make an earlier Worker version live again. It knows nothing of
what your project did around that version: a D1 migration only moves forward, an
agent's stored records carry the schema version of the build that wrote them, and
contexts call each other under contract hashes. `bynk revert` reads those facts
and either rolls back or tells you why not.

Every `bynk deploy` records a compact summary on the Worker version it pushes, in
a plain-text variable named `BYNK_DEPLOY_FACTS`: the migration files per database,
each agent's schema version, the contract hashes it provides and expects, the Durable
Object classes, and the resource ids it binds. It holds no secret. A version
deployed before this existed has no record, so it cannot be reverted: redeploy its
tag instead (`git checkout <tag> && bynk deploy`).

## Revert

```bash
bynk revert --dry-run          # every check, nothing changed
bynk revert                    # confirm, then roll back
```

With no `--to`, the target is what was live just before the release you are
leaving: the deployment immediately before the live code's own upload, secrets
included. A secret you rotated before that release is therefore kept. Reverting
twice goes back past that upload, never to the release you just left. Contexts
revert callers first.

A plain `bynk revert` reverts **every** Worker the lock records for the environment,
each to its own previous deployment, whatever the last release touched. If the last
release changed one context, revert just that one with `--context`; the plan says
so when it applies. `--to <version>` names one Worker's version and needs `--context`.

A dry run reads Cloudflare, so it needs `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`; the token needs read access to D1 as well as Workers.

## What it refuses

Each refusal names its reason, and any check that cannot be answered is a refusal. A deploy or secret change made after the plan also stops the revert before it rolls anything back.

| Check | Refused when | What to do |
|---|---|---|
| `facts` | the live or target version records no deploy facts | Redeploy the previous tag. |
| `d1` | the live version applied migrations the target was not built against | Migrations are forward-only, so the old code may not run on the new schema. If every one is additive, assert it with `--accept-schema-ahead <binding>`. Otherwise redeploy forward. |
| `agent` | the target writes an older schema version than the live one and stored instances exist, are not known to be absent, or the target is below the class's floor | The older code would fault with `NewerSchema` on newer records. Deploy a fix forward. The listing of stored objects lags a write by minutes, so within 30 minutes of the live deploy an empty listing counts as unknown. |
| `layout` | the target writes an older storage layout than the live one (a layout is how an agent's state is placed in storage, recorded in each version's deploy facts) and stored instances exist, or are not known to be absent | The older code cannot read records in the newer layout: a build with the layout check faults them as `NewerLayout`, and one without it would orphan their entries. Deploy a fix forward. No build writes a layout beyond the original yet, so this check does not fire today. |
| `classes` | a Durable Object class was added, removed or renamed between the two versions | Cloudflare cannot roll back across a class migration. |
| `contracts` | the target's contracts no longer match another live Worker, judged as if every selected Worker were reverted together | Revert the whole project, or redeploy forward. See [Contract skew](/book/guides/projects-build-and-deployment/contract-skew/). |
| `ids` | the target binds a KV namespace, database or limiter the lock no longer records | A namespace was re-provisioned since. Redeploy forward. |
| `secrets` | a secret changed after the target was live | A rollback may restore the *old* value of a secret that was rotated since, which peers no longer accept. After a `bynk migrate sweep`, whose two changes touch only its own temporary key, pass `--accept-secret-changes`. |
| `history` | the Worker's history cannot be placed (a Worker first added in the bad release has nothing before it, the live upload is out of the listed history, a gradual deployment is in progress) | Refused for that Worker only; the plan still judges the others. |

`--yes` only skips the confirmation. It never overrides a refusal.

## What it does to `bynk.deploy.lock`

A revert replaces each reverted Worker's recorded contracts with the target's.
Ids, queues and tombstones are untouched. If that changes the file, the command says so;
commit it. Assets need nothing: they belong to the Worker version.

## What it does not do

It never changes stored data, deletes a version, or removes a resource. Cloudflare
keeps a bounded number of versions, so a very old target may not be there. `--prune`
is unchanged: it deletes only orphaned KV namespaces and queues, never data or a Worker.
