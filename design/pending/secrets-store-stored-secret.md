---
level: minor
probes: emit_diagnostics, emit_abi_shapes, ts_writes
changelog: `bynk.cloudflare.StoredSecret` (experimental) reads one Secrets Store secret through a named instance; `bynk deploy` links the store and the secret and never creates them
---

## ADR: linked-account-resources-and-stored-secret
title: Account-level resources are linked by the deploy and never created, starting with Secrets Store secrets
summary: A `linked` flag on instantiable registry kinds, and `StoredSecret` over the Secrets Store binding

**Context.** Cloudflare's Secrets Store binding was the one secret surface
Bynk did not reach. `bynk.Secrets.get(name)` is portable and reads per-Worker
secrets; a Secrets Store secret is account-scoped, shared across Workers,
rotated centrally and bound one secret per binding, so it is Cloudflare-only
(an L3 kind, not a widening of `Secrets`). It is also the first of several
resources a project must *link to* and must not create: the store and the
secret belong to the account and outlive any one deploy. Only a foreign
`Service` Worker behaved this way, with its own warn-only code
(`foreign_worker_warnings`). Proposal #3301 (parent #3184).

**Decision.**

- **A. A new kind, not a wider `Secrets`.** `bynk.cloudflare.StoredSecret` is
  an `Instantiable` with two required options, `@store("name")` and
  `@secret("name")`, binding as `SECRET_<NAME>` in a
  `[[secrets_store_secrets]]` entry. A bare `given StoredSecret` is
  `bynk.capability.instance_only`. Experimental, with an `Option`-free
  `get() -> Effect[String]`: the binding exists only if the deploy found the
  secret, so absence at run time is a platform fault, not a value a handler can
  act on.
- **B. The store is named, never given by id.** The emitted config carries the
  store's *name* in `store_id` (which doubles as its local id under
  `bynk dev`), and `bynk deploy` replaces it with the id it resolves, so ids
  stay out of source. The name is used as written in every environment: a store
  belongs to the account, so it is not environment-qualified.
- **C. `Instantiable::linked`.** A registry flag for kinds the deploy finds and
  never creates. `StoredSecret` is the first to set it. A deploy of a project
  with a linked instance resolves the store by name over Cloudflare's REST API
  (`GET /secrets_store/stores`), checks the secret is in it, and records the
  result in the ledger (`stored_secret_links`, keyed by environment, Worker and
  binding) before the push. A missing store or secret fails the deploy, before
  the confirmation and before anything is provisioned, with every problem named
  at once and the `wrangler secrets-store` command that makes it. Wrangler's
  own `secrets-store` commands print tables, not JSON, so they are not parsed;
  the lookup is the REST client `revert` already uses, needing
  `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, the token with "Secrets
  Store: Read", which a refusal names. Only names and ids cross the wire: a
  secret's value is never requested, so it cannot reach the plan, the ledger,
  generated config or a log. `--dry-run` stays offline, and `--prune` never
  touches a link.
- **D. `Service` is not moved here.** Its warn-only check keeps its own code and
  goldens; moving it onto `linked` is a follow-up.
- **E. Dev and test.** `bynk dev` binds the local store (a production secret
  cannot be read locally) and warns, per served Worker, when a stored secret is
  not in it, with the local `wrangler secrets-store secret create` command.
  Unit and integration tests fault until `stub X.get() returns …`, and the
  `system` tier binds `__makeIntegrationSecretsStoreSecret(binding)`, which
  faults naming the binding. A stubbed value is the test author's own and is
  never printed by the runner.

**Consequences.** mTLS certificates, AI Search instances and verified sender
domains can set `linked` and add a resolver beside `link_stored_secrets`; the
plan and ledger gain a line and a record each. One store name per source means
a project cannot point staging and production at different stores without
different source; a per-environment override is a follow-up if wanted. Rejected:
widening `Secrets` (it would make an L2 capability Cloudflare-shaped); taking
the store by id (it would put an account id in source); warning rather than
failing on a missing secret (a Worker bound to one is broken from its first
request, unlike a Worker whose foreign callee merely arrives late); and
creating the store or secret (the account owns them, and a value must never
pass through Bynk).
