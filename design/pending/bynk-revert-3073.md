---
level: minor
changelog: "`bynk revert` makes a Worker's previous deployment live again without a rebuild, after checking D1 migrations, agent schema versions, contracts, ids and secrets; each deploy records `BYNK_DEPLOY_FACTS` on the version it pushes"
---

## ADR: bynk-revert
title: bynk revert drives Cloudflare's rollback after checks only Bynk can make
summary: Per-version deploy facts travel on the Worker version; every unknown is a refusal

**Context.** After a bad release the options were to redeploy an old tag (a rebuild) or to use Cloudflare's version rollback, which knows nothing of D1 migrations, agents' stored schema versions (ADR 0545, 0558), contract skew (#643, #2077) or the ids the lock records (#3073). The lock holds only what is live, and #3072 stopped it recording derived data, so a revert cannot read the target's facts from it, and there is no build tree on a CI checkout.

**Decision.**
- **A. `bynk revert`, a subcommand, not `deploy --revert`.** It has its own plan and flags, and its dry-run reads Cloudflare, which `deploy --dry-run` promises not to do.
- **B. Cloudflare's own rollback, driven by Bynk** (`wrangler rollback <version> --name <script>`), after the checks. The target is the upload before the live *code* version, not the neighbouring deployment, so a second revert goes further back and never flips to the release just left; rollbacks and secret changes are not steps back. A split (gradual) deployment is refused.
- **C. Facts travel on the version.** Each push sets a plain-text variable `BYNK_DEPLOY_FACTS` (wrangler `--var`, so it needs no config edit and applies to the named environment): compact JSON with the D1 migration files per binding, each agent's schema version, the Durable Object classes, the contracts provided, expected and offered to foreign callers, and the ids bound. It holds no secret. A record over 4,000 bytes is not written, and that version is not revertible. A version without a record, live or target, is refused.
- **D. D1.** Applied migrations are read by a D1 query (`SELECT name FROM d1_migrations`) on the id the facts record, since there is no generated config to run wrangler against. Any applied migration the target was not built against is refused; `--accept-schema-ahead <binding>` is the operator's per-database assertion that they are additive, and `--yes` never implies it. The schema lock covers agent state, not SQL, so Bynk does not judge additivity.
- **E. Agents.** A class whose live schema version exceeds the target's is refused if instances hold stored data, or if that is unknown. It is unknown inside the 30-minute settling margin of the live deploy (ADR 0566) and on a failed listing. A target below the checkout's floor (ADR 0558) is refused. A changed Durable Object class set is refused.
- **F. Contracts.** The target's recorded `provides` and `expects` run through the deploy gate's two checks against the ledger as it will stand once every selected Worker is reverted, so a whole-project revert is not refused for skew between Workers moving together. Callers revert first.
- **G. Ids, secrets, the lock.** A target that binds an id the ledger no longer records is refused. A secret change since the target is refused unless `--accept-secret-changes` (a sweep makes two). The lock's contract records for a reverted Worker are replaced with the target's, written atomically as each Worker lands; ids, queues and tombstones are untouched. Execution re-reads each Worker's live deployment immediately before its rollback and stops if it moved.
- **H. `--prune`** is unchanged: assets and versions are Cloudflare's, and data-holding resources stay list-only (ADR 0255).

**Consequences.** Reverting needs credentials even for a dry run, and a token with D1 read access. The first revertible release is the one that ships this; earlier versions are redeployed by tag. The Cloudflare behaviour this rests on (response shapes of the deployments and version endpoints, what a rollback restores, the size limit of a variable, the objects listing) is encoded behind an `Account` trait and tested against a fake; it has not been exercised against a live account, because no real deploy is run in development. That live probe is the follow-up gate before the feature is described as verified.
