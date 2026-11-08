---
level: patch
changelog: "`bynk.deploy.lock` no longer lists each Worker's static assets by hashed name, so a release that changes only pages leaves the lock unchanged (an existing lock loses its `assets` lines on its next write)"
---

## ADR: deploy-lock-omits-static-assets
title: bynk.deploy.lock does not record static assets
summary: The served asset paths are derived from the build and nothing reads them, so the lock stops listing them

**Context.** ADR 0456 had `bynk deploy` record each Worker's served asset paths in `bynk.deploy.lock`, so that a changed file would be a changed line. The list was write-only: no plan, orphan diff, skew gate, `migrate` or `dev` code read it. Because the paths carry a content hash, every page edit rewrote the lock, and a release run from a tag, which cannot open a pull request, left `main`'s lock stale (#3072). Cloudflare, not the lock, decides what an older Worker version still serves, and no wrangler verb deletes assets, so the list could not support pruning either.

**Decision.** The lock no longer has the per-Worker `assets` field. `DeployLock::record_assets` and `served_assets` are removed. An existing lock that still carries `assets = [...]` reads (unknown keys are ignored) and loses the lines on its next write; `version` stays 1. The plan and its JSON never showed assets and are unchanged. This supersedes the sentence in ADR 0456's Tooling bullet that says `bynk deploy` records the served paths in the lock.

**Consequences.** A release that changes only pages writes an identical lock; the lock changes when a resource is created or a contract changes. The first lock write after upgrading removes the `assets` lines, once. If a later feature needs per-version facts, they travel with the Worker version rather than the committed lock.
