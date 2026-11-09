---
level: patch
changelog: "`bynk build --profile dev|release` and `[profile.dev]`/`[profile.release]` tables in `bynk.toml` choose whether a build carries the function-contract call-site guard; the default stays `release`, the bytes `bynk compile` writes (#3054)"
---

## ADR: bynk-build-profiles
title: Named build profiles for `bynk build`
summary: two fixed profiles, `contracts` the only knob, only `bynk build` reads them

**Context.** `bynk test` and `bynk dev` build with the function-contract call-site guard and `compile`, `deploy` and `tail` without it, so which a project gets is implied by the verb. #3054 settled naming the two as profiles; the first slice (the `bynk build` verb and `[build]`) deferred them because they are hard to reverse once documented.

**Decision.** There are exactly two profiles, `dev` (contracts on) and `release` (off). `bynk build --profile <name>` selects one, `release` when omitted, so a bare `bynk build` stays byte-identical to `bynk compile . -o out`. A `[profile.<name>]` table, whose only key is boolean `contracts`, overrides that profile's default. Precedence is flag over `[profile.<name>]` over `[build]` over the built-in default; `contracts` has no flag and no `[build]` key, so it is the profile's table, else the profile's default. Unknown profile names and keys are refused like every other manifest table. Only `bynk build` reads `[profile.*]`: `bynk test` and `bynk dev` keep contracts on and `compile`, `deploy` and `tail` keep them off, so one manifest line cannot change what ships. A plain release build prints the existing summary line; any other adds its profile and contract state. `[profile.release] contracts = true` is allowed, as the proposal gives the project the choice.

**Consequences.** `test` and `dev` reading `[profile.*]` and `deploy` refusing `--profile dev` are deferred; `deploy` has no `--profile` flag yet. Adding a profile knob later widens the key list and is not breaking; renaming a profile would be.
