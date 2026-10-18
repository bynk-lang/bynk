# 0510 — The editor deploys plan-first and never bypasses the driver's confirmation

- **Status:** Accepted (v0.356.1)

**Context.** #2101 brings `bynk deploy` into VS Code. Deploying is
consequential and outward-facing, so the editor's job is to make the plan
visible and the confirmation deliberate, not to make deploying one click.

**Decision.** **Bynk: Deploy…** picks an environment (those recorded in
`bynk.deploy.lock`, or a new one) and a scope (the project, or one context),
runs `bynk deploy --dry-run --format json`, and renders the plan in the order
the driver will execute it. The plan's Deploy button runs the same arguments,
minus `--dry-run --format json`, as the process of a new terminal.

- The extension never passes `--yes`. The driver's interactive prompt stays the
  one, audited confirmation path (DECISION A). The editor adds a modal that
  names the environment before opening the terminal, but it is not a
  substitute for the driver's prompt.
- `--prune` is not offered. Orphans are listed, with the command to run from a
  terminal (DECISION B).
- The extension never reads, stores or prompts for a secret value. It passes a
  `--secrets-file` path the user chooses, and the driver reads the file
  (DECISION C).
- **Bynk: Show Deploy Plan** renders the same plan with no Deploy button.
- No keybinding is contributed.

All argument lists come from one pure function, so the three rules are
enforced, and unit-tested, in one place. That function also refuses an
environment or context value that could read as a flag (a leading `-`,
whitespace). Command-argument presets are validated the same way, so no caller
can slip a `--yes` in after `--env` or `--context`. Cancelling the dry run
returns silently. A failed refresh keeps the plan view on its last good plan.

**Consequences.** The plan view reads the driver's existing JSON plan, which
the parser treats leniently (unknown fields ignored, missing lists empty), and
which a pinned fixture covers. The plan has no secret-presence field, because
presence is asked of the account only after authenticating, so the view
highlights required (`declared`) secrets rather than claiming which are
missing. A driver refusal before planning (contract skew, a broken pin, a
build error) is shown verbatim from stderr.
