# 0766 — `test` and `deploy` consult `[profile.*]`

- **Status:** Accepted (v0.525.3)

**Context.** #3095 added `[profile.dev|release] contracts` but only `bynk build` read it. #3054 Decision D wanted a project able to turn contracts off for `bynk test`, and a deployment never to carry them; #3107 accepted that.

**Decision.** `bynk-driver::profile_contracts(config, profile)` is the one rule: `[profile.<name>] contracts`, else the profile's default (`dev` on, `release` off). `bynk build` passes its `--profile` (default `release`), `bynk test` and `bynk test --against dev` pass `dev`, so `[profile.dev] contracts = false` turns the guard off in a test run and nothing changes by default. `bynk deploy` always builds `release` and has no `--profile`; since a deployment never carries contract checks (DECISION J of v0.115), it refuses `[profile.release] contracts = true` before any other work, with an error naming the table. That setting still applies to `bynk build`. The refusal is a CLI error, not a diagnostic code.

**Consequences.** The `bynk dev` server (`dev.rs`) builds with contracts off today and does not read the profiles: #3054's text said `bynk dev` builds with contracts on, but that is `bynk test --against dev` (`dev_run.rs`); turning the server's contracts on would change its default, which this change does not do. `bynk tail` and `compile` read neither table.
