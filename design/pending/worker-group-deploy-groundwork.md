---
level: patch
changelog: Deploy's groundwork for a `[deploy.workers]` group (#3562, G4-0): the binding graph is keyed by Worker directory (a group's `name` override and its binding to itself no longer break the order), a group has one merged `bynk-contracts.json` and `bynk-secrets.json`, and a dependency's contract is looked up under the Worker that provides it; `bynk deploy` still refuses a group
probes: ts_writes
---

`bynk deploy` reads each Worker's `wrangler.toml`, `bynk-contracts.json` and
`bynk-secrets.json` from its directory. A group is one Worker, but its members
wrote their manifests under their own prefixes, where deploy would not look, so a
group's contract and secret checks would have silently found nothing. A group now
has one of each: contracts are the members' `provides` plus what they expect of
contexts **outside** the Worker (a call between members is one build and one
upload and has no contract to skew), and secrets are the union, complete only if
every member's census is. The deploy graph reads each binding's target by script
name and keys it by Worker directory, drops a Worker's binding to itself
(ADR 0874 D7), and a dependency's contract is found under its group's directory.
Nothing changes for a project with no group, and `bynk deploy` still refuses one
until G4-1.
