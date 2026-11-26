---
level: minor
probes: diagnostic_coverage
changelog: A `D1` capability takes `@jurisdiction("eu" | "fedramp" | "us")`, and `bynk deploy` creates its database with `--jurisdiction`; changing it on a database already provisioned is refused, never recreated
---

## ADR: d1-jurisdiction
title: A D1 database is pinned to a Cloudflare jurisdiction at creation, and changing it after is refused
summary: D1 shares R2's and the agent's value set; the ledger records the jurisdiction, and the plan refuses a change instead of recreating a database that holds data
**Context.** #3820. `@jurisdiction` was accepted on `R2` and (#3819) on agents, but a `D1` database could not be created in a Cloudflare jurisdiction, so relational data could not be kept in the EU. The jurisdiction of a D1 database is fixed by `wrangler d1 create --jurisdiction` and cannot be changed afterwards.

**Decision.** (A) `D1` takes the same optional `@jurisdiction("…")` as `R2`, through the one shared value set (`bynk_std::JURISDICTIONS`: `"eu"`, `"fedramp"`, `"us"`) and the one checker helper, so a bad value is `bynk.capability.invalid_option` for every kind. The build carries it on `BindingSource::D1Database`, the deploy on `D1Decl`, and `create_d1` passes `--jurisdiction <j>`. The plan line reads `d1 create <db> (<binding>) in jurisdiction eu`. The ledger's `D1Record` records the jurisdiction (absent for none, so an older lock reads as unrestricted). (B) A database already in the ledger under its derived name whose recorded jurisdiction differs from the source's is refused at plan time (the existing database-name refusal hook), with a message that says why and what to do. It is never recreated: a new database would orphan the rows, and an unconfirmed destructive path is worse than a refusal. A database that is adopted by name (not in the ledger) keeps whatever jurisdiction it was made with, which Bynk cannot read back.

**Consequences.** `capability Orders = D1 @jurisdiction("eu")` creates the database in the EU. Whether Cloudflare honours the flag needs a live account, which this change does not use: tests cover the checker, the `--jurisdiction` argument, the plan line, the ledger entry and the refusal.
