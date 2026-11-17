---
level: minor
probes: diagnostic_coverage, explained_codes
changelog: `bynk revert` reverts a `[deploy.workers.<key>]` group as one Worker (`--worker KEY`), so every member goes back together; a member named with `--context` is refused, naming the Worker that holds it (#3562, G4-3)
---

A group is one Cloudflare script, so `bynk revert` rolls it back as one: the same
version rollback it does for any Worker, which puts every member back atomically.
`--worker NAME` reverts one Worker by its directory name (a group's key, or a
context's dashed name; not with `--context`), and `--to` takes either selector.
`--context` for a group member is refused with the Worker that holds it. The
dependency lookup that orders reverts and checks contract skew finds a context under
the group the ledger recorded it in, a renamed group's script is rolled back under
its script name, and a call between members is never a skew (the merged manifest
holds no expectation of a member). `bynk dev` still refuses a group (G6).
