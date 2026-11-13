---
level: minor
changelog: `bynk deploy` links each `AiSearch` instance by name, records it in the ledger and fails when it is missing, never creating one; `bynk dev` proxies the binding to the deployed instance with a one-time warning, silenced by `[dev] ai-search = "allow"`
probes: emit_diagnostics
---

## ADR: ai-search-deploy-and-dev
title: AI Search instances are linked by the deploy, and bynk dev proxies them with a one-time warning
summary: Slice AS2 of #3480: the deploy looks each instance up and fails when it is missing, and bynk dev warns once per binding kind and proceeds

**Context.** An AI Search instance is owned by someone else, so the deploy cannot create it (#3301's rule, ADR 0802), and it has no local simulation, so `bynk dev` can only reach the deployed one.

**Decision.** The deploy resolves each `AiSearch` instance over the REST API (`GET …/ai-search/namespaces/default/instances/{name}`) after authenticating and before anything is provisioned, records `{instance}` per environment, Worker and binding in a new ledger section beside `stored_secret_links`, and fails, naming every missing instance at once and saying the deploy never creates one. A paused or disabled instance (the documented `paused` and `enable` flags) is a warning, not a failure, because it still answers; the `status` values are not documented, so they are not interpreted. The plan lists a `linked ai-search` line with no action. The token needs the permissions Cloudflare names for every AI Search REST call (Edit and Run); the docs list no read-only one, so the failure message names both. `bynk dev` follows ADR 0834: it never blocks, the first call of a session prints one warning linking to Cloudflare's pricing page, and `[dev] ai-search = "allow"` silences it. The kind is separate from `ai` (`BYNK_DEV_ACK_AI_SEARCH`). Unlike Workers AI, AI Search needs the binding proxied, so `bynk dev` writes `remote = true` into the `[[ai_search]]` entries of the development copy of each Worker's config only; the generated config the deploy reads is unchanged. In `bynk test` every operation still faults until stubbed. Instance names are used as written in every environment, and an instance name is checked at compile time against Cloudflare's documented form (1 to 64 lowercase letters, digits, hyphens and underscores).

**Consequences.** A project with no `AiSearch` instance deploys and serves exactly as before. Staging reads the production instance, since names are not environment-qualified. Unverified until a real-account check: the shape of the lookup's `result` beyond `paused` and `enable`, and whether a narrower token permission suffices.
