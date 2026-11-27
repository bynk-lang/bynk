# 1076 — The refusals a deploy makes after it derives the plan are entries of the JSON plan too

- **Status:** Accepted (v0.807.2)

**Context.** #3797, part 3. Parts 1 and 2 made the plan document carry `refusals` and `warnings` and converted the gates before the plan. The gates after it still printed a stderr line and returned an exit code, so a client asked for JSON got no entry for them. The `diagnostic_coverage` probe also counted seven deploy codes from part 2 as unasserted.

**Decision.** (A) The remaining plan-stage refusals return their entry through `Stop`, or onto the plan for a refusal made once it exists (`Plan::with_refusal`). New codes: `pins_unreadable`, `account_name_conflict`, `secrets_file_passthrough`, `strict_environments_shared`, `strict_environments_unchecked`, `hyperdrive_settings_unreadable`, `hyperdrive_unresolved`, `recreate_needs_yes`; the existing `contract_skew` and `foreign_contract_skew` gain subject fields (`side`, `worker`, `skews`; `pins`). (B) The entries are built by pure constructors in `plan/refusals.rs`, each asserted by a unit test that names the code as a literal, which backfills the seven part-2 codes the probe saw as unasserted: `contracts_lock_unwritable`, `contracts_manifest_unreadable`, `email_claim_refused`, `preflight_failed`, `rebuild_failed`, `upload_order_cycle` and `workers_unreadable`. (C) A failed preflight's entry carries the doctor's pinned JSON as `capabilities`. (D) The stderr lines are byte-for-byte what they were.

**Consequences.** Additive for a JSON consumer. The account checks, confirmations and the remaining warnings stay stderr-only until the next increment; `bynk revert --format json` verdicts and `bynk.cloudflare.credentials_missing` follow in part 4.
