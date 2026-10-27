---
level: minor
changelog: "VS Code: a State Migrations node in the Bynk sidebar shows `bynk migrate status` per agent class, and **Bynk: Sweep State Migrations…** runs `bynk migrate sweep` plan-first (#2468)"
---

## ADR: vscode-plan-first-migration-sweep
title: The editor sweeps state migrations plan-first, with a typed confirmation for production
summary: The sweep plan is the offline status report; the driver's prompt stays the confirmation, and production adds a typed phrase

**Context.** #2468 brings `bynk migrate` into VS Code. A sweep changes the
account: it sets and deletes a temporary secret on every Worker (each creating a
Worker version) and wakes every stored instance. Unlike `bynk deploy`, the
driver has no dry run, and what a sweep will touch, the instance counts, comes
only from Cloudflare's API. The extension never calls Cloudflare itself.

**Decision.** The Project sidebar gains a **State Migrations** node per project,
fed by `bynk migrate status --format json` (offline; the driver already had the
mode, and a driver test now pins its document against the extension's fixture).
Each class shows its version and floor, its `migrate` steps (those the floor has
passed marked retirable), versions with no path, and its environments; selecting
it opens the agent state view (ADR 0504's `bynk.showAgentState`).

**Bynk: Sweep State Migrations…** picks an environment (or every recorded one)
and a scope, then shows a plan built from the status report: the classes, the
environments, and whether the run can raise each class's floor. Its Sweep button
runs `bynk migrate sweep` in a terminal without `--yes`, so the driver lists the
instances and its own prompt stays the one audited confirmation, as for deploy
(ADR 0510). Before the terminal opens, a sweep that touches production needs a
typed confirmation: the environment's name, or `all environments`. The manifest
has no production marker for the sweep to read, so production is decided by name:
`default`, `prod`, `production` or `live`, case-insensitively. `--batch` and
`--url` are left to a terminal.

**Consequences.** The plan cannot show per-environment instance counts: they need
the account, so the driver prints them in the terminal, before it asks. A
production environment named otherwise (`eu-main`) is not typed-confirmed; the
driver's prompt still applies. The same argument builder serves the plan and the
run, so `--yes` cannot be passed from the editor, and the unit tests pin that.
