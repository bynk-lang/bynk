# 0512 — The editor renders the doctor's verdict and offers it at the point of failure

- **Status:** Accepted (v0.356.3)

**Context.** #2095: when a tool is missing (no `bynk`, an old Node, no
`wrangler`, `bynkc` version skew), the extension failed at the point of use with
a raw spawn or CLI message. `bynk doctor` already diagnoses all of this, prints
the remedy, and declares `--format json` its stable scriptable surface.
Re-implementing its probes in TypeScript would drift.

**Decision.** **Bynk: Doctor** runs `bynk doctor --format json` (with `--only`
when the caller knows the capability) and renders the report in a webview,
grouped by capability with ok, warn and fail marks (DECISION A). The view is
host-rendered through the same CSP and nonce posture as the other webviews.

Each gap's remedy offers buttons: run the command it names in a new, visible
terminal after a modal confirmation, open a link it names, open the matching
setting (`bynk.executablePath`, `bynk.compilerPath`), or copy it (DECISION B).
The host runs only a command the current report offers, and never silently.

The doctor is not run on activation (DECISION C). It is offered as a "Run
Doctor" action where a failure points at the toolchain: a test run that can't
start or crashes (`--only test`), a failed debug launch (`--only test` or
`--only deploy`), and `bynk dev`'s pre-flight refusal (`--only deploy`). When
`bynk` itself can't be started, the doctor can't run either, so that case gets
a fixed message with install guidance and the `bynk.bynkPath` setting. This
covers the dev server, deploy, the Test Explorer, debugging, and the
`bynkc: check` task's exit 127. The walkthrough gains a "Check your setup"
step.

**Consequences.** The parser is lenient (unknown fields ignored, an unknown
level read as `warn`), and its unit tests read the driver's own golden
`bynk/tests/golden/doctor.json`, so a change to the doctor's JSON shows up in
both suites. Commands are recognised in a remedy by their leading program
(`npm`, `npx`, `cargo`, `rustup`, …). A prose remedy offers only copy, a link
or a setting.
